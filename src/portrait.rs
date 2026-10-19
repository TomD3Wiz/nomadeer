use std::time::Duration;

use eframe::egui::ColorImage;
use reqwest::blocking::Client;

const USER_AGENT: &str = "Nomadeer/0.1 private wormhole mapper";

pub(crate) fn fetch_character_portrait(character_id: &str) -> Result<ColorImage, String> {
    let url = format!("https://images.evetech.net/characters/{character_id}/portrait?size=128");
    let response = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("Could not start portrait request: {error}"))?
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .map_err(|error| format!("Could not reach EVE image server: {error}"))?;

    let status = response.status();
    let bytes = response
        .bytes()
        .map_err(|error| format!("Could not read portrait response: {error}"))?;
    if !status.is_success() {
        return Err(format!("EVE image server returned {status}"));
    }

    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("Could not decode character portrait: {error}"))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}
