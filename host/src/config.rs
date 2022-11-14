use crate::State;

use anyhow::{Context, Result};
use ron::{de::from_reader, ser::to_writer_pretty};
use uuid::Uuid;
use xdg::BaseDirectories;

use std::{collections::HashMap, fs::File};

pub fn load_config() -> Result<State> {
    let dirs = BaseDirectories::new().context("No HOME")?;
    match dirs.find_config_file("sunrise.ron") {
        Some(path) => {
            let file = File::open(&path)
                .with_context(|| format!("Unable to open config file at: {}", path.display()))?;
            from_reader(&file).with_context(|| format!("Unable to parse config file."))
        }
        None => {
            let path = dirs
                .place_config_file("sunrise.ron")
                .context("Unable to place config file")?;
            let file = File::create(path).context("Unable to write config file")?;
            let state = generate_new_state()?;
            to_writer_pretty(file, &state, Default::default())
                .context("Unable to serialize config")?;
            Ok(state)
        }
    }
}

pub fn save_config(state: &State) -> Result<()> {
    let dirs = BaseDirectories::new().context("No HOME")?;
    let path = dirs.get_config_file("sunrise.ron");
    let file = File::create(path).context("Unable to write config file")?;
    to_writer_pretty(file, state, Default::default()).context("Unable to serialize config")?;
    Ok(())
}

fn generate_new_state() -> Result<State> {
    let (cred, key) = crate::crypto::gen_creds().context("Generation certificate failed")?;

    Ok(State {
        unique_id: Uuid::new_v4(),
        server_cert: cred,
        server_key: key,
        known_clients: HashMap::new(),
        apps: Vec::new(),

        hostname: hostname::get()
            .ok()
            .and_then(|host| host.into_string().ok())
            .unwrap_or_else(|| String::from("Sunrise")),
        interface: crate::serialization::get_default_interface(),
        http_port: 47989,
        https_port: 47984,

        max_sessions: 1,
        sessions: HashMap::new(),
    })
}
