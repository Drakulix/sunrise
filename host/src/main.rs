#![recursion_limit = "256"]

use anyhow::Result;
use default_net::Interface;
use gotham::{router::response::StaticResponseExtender, state::StateData};
use openssl::{
    pkey::{PKey, Private},
    x509::X509,
};
use serde::{Deserialize, Serialize};
use simplelog::*;
use uuid::Uuid;

use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

//pub mod compositor;
pub mod config;
pub mod crypto;
pub mod http;
pub mod rtsp;
pub mod serialization;

#[derive(StateData, Debug, Clone)]
pub struct SharedState(Arc<Mutex<State>>);
impl std::panic::RefUnwindSafe for SharedState {}

#[derive(
    Debug, Serialize, Deserialize, StateData, StaticResponseExtender, PartialEq, Eq, Hash, Clone,
)]
pub struct ClientInfo {
    uniqueid: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct State {
    unique_id: Uuid,
    #[serde(with = "serialization::cert")]
    server_cert: X509,
    #[serde(with = "serialization::key")]
    server_key: PKey<Private>,
    known_clients: HashMap<ClientInfo, Client>,
    apps: Vec<App>,

    hostname: String,
    #[serde(skip, default = "serialization::get_default_interface")]
    interface: Interface,
    http_port: u16,
    https_port: u16,

    max_sessions: usize,
    #[serde(skip)]
    sessions: HashMap<Uuid, Session>,
}

#[derive(Debug)]
pub struct Session {
    app: AppId,
    client: Client,
    rikey: String,
    rikeyid: String,
    /*
    rtsp_port: u16,
    ctrl_port: u16,
    video_port: u16,
    audio_port: u16,
    */
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AppId(u64);
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Client {
    paired: bool,
    #[serde(with = "serialization::cert")]
    client_cert: X509,
    key: Vec<u8>,
    server_secret: Option<[u8; 16]>,
    server_challenge: Option<[u8; 16]>,
    client_hash: Option<Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct App {
    title: String,
    command: String,
    asset: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    openssl::init();
    let _ = TermLogger::init(
        if cfg!(debug_assertions) {
            LevelFilter::Info
        } else {
            LevelFilter::Warn
        },
        Config::default(),
        TerminalMode::Stderr,
        ColorChoice::Auto,
    );

    let config = config::load_config()?;
    let state = SharedState(Arc::new(Mutex::new(config)));
    let http_state = http::init(state.clone()).await?;
    tokio::select! {
        biased;

        _ = http_state.http_server => {},
        _ = http_state.https_server => {},
    };

    Ok(())
}
