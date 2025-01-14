#[allow(unused_must_use)]

extern crate captrs;

use captrs::*;
use std::{time::Duration};
use console::Emoji;
use url::Url;
use hass_rs::{client, HassClient, WSEvent};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Serialize, Deserialize)]
struct Settings {
    api_endpoint: String,
    light_entity_name: String,
    trigger_entity_name: String,
    token: String,
    transition: f32,
    grab_interval: i16,
    skip_pixels: i16,
    smoothing_factor: f32,
    monitor_id: i16,
}

#[derive(Serialize, Deserialize)]
struct HASSApiBody {
    entity_id: String,
    rgb_color: [u64; 3],
    brightness: u64,
}

#[tokio::main]
async fn main() {
    let term = console::Term::stdout();
    term.set_title("HASS-Light-Sync running...");
    
    println!("{}hass-light-sync - Starting...", Emoji("💡 ", ""));
    println!("{}Reading config...", Emoji("⚙️ ", ""));
    // read settings
    let settingsfile =
        std::fs::read_to_string("settings.json").expect("❌ settings.json file does not exist");


    let settings: Settings =
        serde_json::from_str(settingsfile.as_str()).expect("❌ Failed to parse settings. Please read the configuration section in the README");

    println!("{}Config loaded successfully!", Emoji("✅ ", ""));

    println!("{}Connecting to HASS ... ", Emoji("💡 ", ""));
    let mut client: HassClient;
    let connect = client::connect_to_url(Url::parse(settings.api_endpoint.as_str()).unwrap()).await;
    match connect {
        Ok(_res_con) => {
            // Connection succeed, let's authenticate
            client = _res_con;
            let _res_auth = client.auth_with_longlivedtoken(&*settings.token).await;
            match _res_auth{
                Ok(_res_auth) => {
                    // Authentication succeed
                    println!("{}Connected !", Emoji("✅ ", ""));
                },
                Err(e) => {
                    // Authentication failed
                    println!("{}Connection to Home Assistant failed: {}", Emoji("❌ ", ""), e);
                    std::process::exit(0);
                }
            }
        },
        Err(e) => {
            // Connection failed
            println!("{}Connection to Home Assistant failed: {}", Emoji("❌ ", ""), e);
            std::process::exit(0);
        }
    }

    let enable: Arc<Mutex<bool>> = Arc::new(Mutex::from(false));
    let cl_enable = Arc::clone(&enable);
    let cl_trigger_entity_name = settings.trigger_entity_name.clone();
    let callback = move |item: WSEvent| {
        if item.event.data.entity_id != cl_trigger_entity_name {
            return;
        }
        let new_state: String;
        match item.event.data.new_state {
            Some(p) => new_state=p.state,
            None => new_state="None".to_string(),
        }

        *cl_enable.lock().unwrap() = new_state == "on";
        println!(
        "Event : id {}, at {}, entity: {}, new_state: {}", item.id, item.event.time_fired, item.event.data.entity_id, new_state );
    };
    match client.subscribe_event("state_changed", callback).await {
        Ok(v) => println!("{}Event subscribed : {}", Emoji("✅ ", ""), v),
        Err(err) => println!("Oh no, an error: {}", err),
    }


    /*let tmp_avg_arr = vec![100, 0, 0];

    // get the highest rgb component value -> brightness
    let tmp_brightness = tmp_avg_arr.iter().max().unwrap();
    send_rgb(&mut client, &settings, &tmp_avg_arr, tmp_brightness).await;
    return;*/

    let steps = settings.skip_pixels as u64;
    let grab_interval = settings.grab_interval as u64;
    let smoothing_factor = settings.smoothing_factor;

    // create a capture device
    let mut capturer =
        Capturer::new(settings.monitor_id as usize)
            .expect("❌ Failed to get Capture Object");

    // get the resolution of the monitor
    let (w, h) = capturer.geometry();
    let size = (w as u64 * h as u64) / steps;

    let (mut prev_r, mut prev_g, mut prev_b) = (0, 0, 0);
    
    println!();

    let mut last_timestamp = std::time::Instant::now();

    loop {
        // Lock enable variable and read the status (edited from closure above)
        let enable = *enable.lock().unwrap();
        if !enable {
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        // allocate a vector array for the pixels of the display
        let ps: Vec<Bgr8>;

        // try to grab a frame and fill it into the vector array, if successful, otherwise sleep for 100 ms and skip this frame.
        match capturer.capture_frame() {
            Ok(res) => ps = res,
            Err(error) => {
                println!("{} Failed to grab frame: {:?}", Emoji("❗ ", ""), error);
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        }

        let (mut total_r, mut total_g, mut total_b) = (0, 0, 0);

        let mut count = 0;

        // for every nth pixel, add the rgb value
        for Bgr8 { r, g, b, .. } in ps.into_iter() {
            if count % steps == 0 {
                total_r += r as u64;
                total_g += g as u64;
                total_b += b as u64;
            }
            count += 1;
        }

        // calculate avg colors
        let (avg_r, avg_g, avg_b) = (total_r / size, total_g / size, total_b / size);

        // smoothing
        let (sm_r, sm_g, sm_b) = (
            smoothing_factor * prev_r as f32 + (1.0 - smoothing_factor) * avg_r as f32,
            smoothing_factor * prev_g as f32 + (1.0 - smoothing_factor) * avg_g as f32,
            smoothing_factor * prev_b as f32 + (1.0 - smoothing_factor) * avg_b as f32,
        );

        // store into prev
        prev_r = sm_r as u64;
        prev_g = sm_g as u64;
        prev_b = sm_b as u64;

        // put into vector
        let avg_arr = vec![prev_r, prev_g, prev_b];

        // get the highest rgb component value -> brightness
        let brightness = avg_arr.iter().max().unwrap();

        let time_elapsed = last_timestamp.elapsed().as_millis();
        last_timestamp = std::time::Instant::now();
        term.clear_line();
        println!("{}Current average color: {:?} - Brightness: {} - FPS: {}", Emoji("💡 ", ""), avg_arr, brightness, 1000 / time_elapsed);
        // println!("Avg Color: {:?}    Brightness: {}", avg_arr, brightness);
        send_rgb(&mut client, &settings, &avg_arr, brightness).await;
        std::thread::sleep(Duration::from_millis(grab_interval));
    }
}



async fn send_rgb(
    client: &mut HassClient,
    settings: &Settings,
    rgb_vec: &std::vec::Vec<u64>,
    brightness: &u64,
) {
    let api_body = json!({
        "entity_id": String::from(settings.light_entity_name.as_str()),
        "rgb_color": [rgb_vec[0], rgb_vec[1], rgb_vec[2]],
        "brightness": *brightness,
        "transition": settings.transition
    });

    match client.call_service(
        "light".to_owned(),
        "turn_on".to_owned(),
        Some(api_body)
    ).await {
        Ok(_v)=>(),
        Err(err) => {
            println!("{}Connection to Home Assistant failed: {}", Emoji("❌ ", ""), err);
            std::process::exit(0);
        }
    }
}
