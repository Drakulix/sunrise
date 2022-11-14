use anyhow::{Context, Result};
use default_net::interface::MacAddr;
use format_xml::xml;
use gotham::{
    prelude::*,
    rustls::Certificate,
    state::{client_addr, State},
};
use openssl::{md::Md, rand::rand_bytes, sha::Sha256, x509::X509};
use serde::Deserialize;
use tokio::sync::mpsc::Sender;
use uuid::Uuid;

use super::AddCert;
use crate::{
    config::save_config, AppId, Client, ClientInfo, Session, SharedState, State as RawState,
};
use std::time::Duration;

const VERSION: &'static str = "7.1.431.0";
const GFE_VERSION: &'static str = "3.23.0.74";

pub async fn server_info(mut state: State) -> (State, String) {
    let info = ClientInfo::take_from(&mut state);

    let resp = {
        let raw_state = SharedState::borrow_from(&state).0.clone();
        let config = raw_state.lock().await;

        let (client, is_paired, session) = if let Some(client) = config.known_clients.get(&info) {
            let is_paired = client.paired;
            let session = config
                .sessions
                .values()
                .find(|session| &session.client == client);
            (Some(client), is_paired, session)
        } else {
            (None, false, None)
        };

        xml! {
            <root status_code=200>
                <hostname>{config.hostname}</hostname>
                <appversion>{VERSION}</appversion>
                <GfeVersion>{GFE_VERSION}</GfeVersion>
                <uniqueid>{config.unique_id}</uniqueid>
                <HttpsPort>{config.https_port}</HttpsPort>
                <ExternalPort>{config.http_port}</ExternalPort>
                <mac>{config.interface.mac_addr.as_ref().unwrap_or(&MacAddr::zero())}</mac>
                <MaxLumaPixelsHEVC>0</MaxLumaPixelsHEVC>
                <LocalIP>{config.interface.ipv4[0].addr}</LocalIP>
                <ServerCodecModeSupport>3</ServerCodecModeSupport>
                <SupportedDisplayMode>
                    <DisplayMode>
                        <Width>1920</Width>
                        <Height>1080</Height>
                        <RefreshRate>60</RefreshRate>
                    </DisplayMode>
                </SupportedDisplayMode>
                <PairStatus>{if is_paired { 1 } else { 0 }}</PairStatus>
                <currentgame>{session.map(|x| x.app.0).unwrap_or(0)}</currentgame>
                <state>{session.map(|_| "SUNSHINE_SERVER_BUSY").unwrap_or("SUNSHINE_SERVER_FREE")}</state>
            </root>
        }
        .to_string()
    };

    (state, resp)
}

pub async fn http_pair(mut state: State) -> (State, String) {
    let pairing_query = PairingQueryExtractor::take_from(&mut state);
    let config = SharedState::borrow_from(&state);
    let sender = AddCert::borrow_from(&state);

    let result = {
        let mut raw_state = config.0.lock().await;
        let client_info = ClientInfo {
            uniqueid: pairing_query.uniqueid.clone(),
        };

        let result = match pairing_query.try_into() {
            Ok(PairingVariant::GetServerCert { salt, clientcert }) => {
                get_server_cert(&mut raw_state, client_info, salt, clientcert).await
            }
            Ok(PairingVariant::ClientChallenge { clientchallenge }) => {
                client_challenge(&mut raw_state, client_info, clientchallenge)
            }
            Ok(PairingVariant::ServerChallengeResp {
                serverchallengeresp,
            }) => server_challenge_response(&mut raw_state, client_info, serverchallengeresp),
            Ok(PairingVariant::ClientPairingSecret {
                clientpairingsecret,
            }) => {
                client_pairing_secret(
                    &mut raw_state,
                    client_info,
                    clientpairingsecret,
                    &sender.add_cert,
                )
                .await
            }
            Err(()) => Err(anyhow::anyhow!("Unknown pairing request")),
        };

        let _ = save_config(&raw_state);

        result
    };

    match result {
        Ok(resp) => (state, resp),
        Err(err) => {
            log::error!("Error: {}", err);
            (
                state,
                xml! {
                    <root status_code=200>
                        <paired>0</paired>
                    </root>
                }
                .to_string(),
            )
        }
    }
}

pub async fn https_pair(mut state: State) -> (State, String) {
    let client_info = ClientInfo::take_from(&mut state);
    let config = SharedState::borrow_from(&state);
    let resp = {
        let mut raw_state = config.0.lock().await;
        log::info!("PAIRED: {:?}!", client_info);

        match raw_state
            .known_clients
            .get_mut(&client_info)
            .with_context(|| format!("Failed to find client for id: {:?}", client_info))
        {
            Ok(mut client) => {
                client.paired = true;
                let _ = save_config(&raw_state);

                xml! {
                    <root status_code=200>
                        <paired>1</paired>
                    </root>
                }
                .to_string()
            }
            Err(_) => xml! {
                <root status_code=400>
                    <paired>0</paired>
                </root>
            }
            .to_string(),
        }
    };

    (state, resp)
}

pub async fn applist(mut state: State) -> (State, String) {
    let info = ClientInfo::take_from(&mut state);
    let config = SharedState::borrow_from(&state);
    let resp = {
        let raw_state = config.0.lock().await;

        if !raw_state.known_clients.contains_key(&info) {
            xml! { <root status_code=501 /> }.to_string()
        } else {
            xml! {
                <root status_code=200>
                for (i, app) in (raw_state.apps.iter().enumerate()) {
                    <App>
                        <IsHdrSupported>0</IsHdrSupported>
                        <AppTitle>{app.title}</AppTitle>
                        <ID>{i+1}</ID>
                    </App>
                }
                </root>
            }
            .to_string()
        }
    };

    (state, resp)
}

pub async fn launch(mut state: State) -> (State, String) {
    let args = LaunchQueryExtractor::take_from(&mut state);
    let info = ClientInfo {
        uniqueid: args.uniqueid.clone(),
    };
    let config = SharedState::borrow_from(&state);
    let addr = client_addr(&state).expect("no client address");

    let resp = {
        let mut raw_state = config.0.lock().await;
        if raw_state.apps.get(args.appid - 1).is_some() {
            let rtsp_listener = crate::rtsp::init().await.unwrap();
            let rtsp_port = rtsp_listener.local_addr().unwrap().port();

            let id = Uuid::new_v4();
            let session = Session {
                app: AppId((args.appid - 1) as u64),
                client: raw_state.known_clients.get(&info).unwrap().clone(),
                rikey: args.rikey,
                rikeyid: args.rikeyid,
            };
            raw_state.sessions.insert(id.clone(), session);

            let move_state = config.clone();
            tokio::spawn(async move {
                if let Ok(Ok((stream, addr))) =
                    tokio::time::timeout(Duration::from_secs(30), rtsp_listener.accept()).await
                {
                    log::info!("RTSP Connection from: {}", addr);
                    crate::rtsp::new_client(rtsp_listener, stream, move_state, id).await;
                } else {
                    // TODO: we didn't even make it to the start, discard session
                }
            });

            // TODO, find free ports (just use 0? and query tokio?)
            // Launch tasks for all of them
            // Add keys, joinhandles,  to session struct
            // launch compositor
            // launch sockets
            // answer client

            let ip = "127.0.0.1"; //addr.ip();
            let url = format!("rtsp://{ip}:{rtsp_port}");

            xml! {
                <root status_code=200>
                    <sessionUrl0>{url}</sessionUrl0>
                    <gamesession>1</gamesession>
                </root>
            }
            .to_string()
        } else {
            // app does not exist
            xml! {
                <root status_code=400>
                    <gamesession>0</gamesession>
                </root>
            }
            .to_string()
        }
    };

    (state, resp)
}

pub async fn unpair(mut state: State) -> (State, String) {
    let info = ClientInfo::take_from(&mut state);
    let config = SharedState::borrow_from(&state);

    {
        let mut raw_state = config.0.lock().await;
        raw_state.known_clients.remove(&info);
        let _ = save_config(&raw_state);
    }

    (
        state,
        xml! {
            <root status_code=200>
                <paired>0</paired>
            </root>
        }
        .to_string(),
    )
}

async fn get_server_cert(
    state: &mut RawState,
    client_id: ClientInfo,
    salt: String,
    client_cert: String,
) -> Result<String> {
    let salt = hex::decode(salt.into_bytes()).context("Unable to decode salt")?;

    // read pin from command line
    let pin = tokio::task::spawn_blocking(|| {
        let mut rl = rustyline::Editor::<()>::new()?;
        rl.readline("Pin: ")
    })
    .await??;
    let key = crate::crypto::gen_aes_key(&salt, &pin);

    let client_cert = client_cert.into_bytes();
    let decoded = hex::decode(client_cert).context("Unable to decode client certificate")?;
    log::debug!("client_cert: {:?}", std::str::from_utf8(&decoded));
    let client_cert = X509::from_pem(&decoded)?;

    state
        .known_clients
        .entry(client_id)
        .and_modify(|client| {
            client.client_cert = client_cert.clone();
            client.key = key.clone();
        })
        .or_insert_with(|| Client {
            paired: false,
            client_cert,
            key,
            server_challenge: None,
            server_secret: None,
            client_hash: None,
        });

    let server_cert = state.server_cert.to_pem()?;
    log::debug!("server_cert: {:?}", std::str::from_utf8(&server_cert));
    let server_cert = hex::encode(server_cert);

    Ok(xml! {
        <root status_code=200>
            <paired>1</paired>
            <plaincert>{server_cert}</plaincert>
        </root>
    }
    .to_string())
}

fn client_challenge(
    state: &mut RawState,
    client_id: ClientInfo,
    challenge: String,
) -> Result<String> {
    let challenge =
        hex::decode(challenge.into_bytes()).context("Unable to decode client challenge")?;

    let mut client = state
        .known_clients
        .get_mut(&client_id)
        .with_context(|| format!("Failed to find client for id: {:?}", client_id))?;

    let decrypted = crate::crypto::aes_decrypt_ecb(&challenge, &client.key, false)
        .context("Unable to decrypt client challenge")?;
    let signature = state.server_cert.signature().as_slice();
    let mut secret = [0; 16];
    rand_bytes(&mut secret)?;

    let mut hasher = Sha256::new();
    hasher.update(&decrypted);
    hasher.update(&signature);
    hasher.update(&secret);
    let hash = Vec::from(hasher.finish());

    let mut server_challenge = [0; 16];
    rand_bytes(&mut server_challenge)?;

    let mut plaintext = Vec::new();
    plaintext.extend(&hash);
    plaintext.extend(&server_challenge);

    let encrypted = crate::crypto::aes_encrypt_ecb(&plaintext, &client.key, false)
        .context("Unable to encode response")?;
    let response = hex::encode(encrypted);
    client.server_secret = Some(secret);
    client.server_challenge = Some(server_challenge);

    Ok(xml! {
        <root status_code=200>
            <paired>1</paired>
            <challengeresponse>{response}</challengeresponse>
        </root>
    }
    .to_string())
}

fn server_challenge_response(
    state: &mut RawState,
    client_id: ClientInfo,
    challenge: String,
) -> Result<String> {
    let challenge =
        hex::decode(challenge.into_bytes()).context("Unable to decode client challenge")?;

    let mut client = state
        .known_clients
        .get_mut(&client_id)
        .with_context(|| format!("Failed to find client for id: {:?}", client_id))?;

    let decrypted = crate::crypto::aes_decrypt_ecb(&challenge, &client.key, false)
        .context("Unable to decrypt client challenge")?;
    client.client_hash = Some(decrypted);

    if let Some(secret) = client.server_secret.as_ref() {
        let signed = crate::crypto::sign(&state.server_key, secret, Md::sha256())?;
        assert!(crate::crypto::verify(
            &state.server_cert,
            secret,
            &signed,
            Md::sha256(),
        )?);
        let mut pairingsecret = Vec::from(secret.as_slice());
        pairingsecret.extend(signed);
        let pairingsecret = hex::encode(pairingsecret);

        Ok(xml! {
            <root status_code=200>
                <paired>1</paired>
                <pairingsecret>{pairingsecret}</pairingsecret>
            </root>
        }
        .to_string())
    } else {
        Ok(xml! {
            <root status_code=200>
                <paired>0</paired>
            </root>
        }
        .to_string())
    }
}

async fn client_pairing_secret(
    state: &mut RawState,
    client_id: ClientInfo,
    client_pairing_secret: String,
    verifier: &Sender<Certificate>,
) -> Result<String> {
    let client_secret = hex::decode(client_pairing_secret.into_bytes())
        .context("Unable to decode client pairing secret")?;

    let secret = &client_secret[0..16];
    let sign = &client_secret[16..];

    let client = state
        .known_clients
        .get_mut(&client_id)
        .with_context(|| format!("Failed to find client for id: {:?}", client_id))?;

    if let (Some(challenge), Some(client_hash)) = (
        client.server_challenge.as_ref(),
        client.client_hash.as_ref(),
    ) {
        let signature = client.client_cert.signature().as_slice();
        let mut hasher = Sha256::new();
        hasher.update(challenge.as_slice());
        hasher.update(&signature);
        hasher.update(&secret);
        let hash = Vec::from(hasher.finish());

        if &hash == client_hash
            && crate::crypto::verify(&client.client_cert, secret, sign, Md::sha256())?
        {
            verifier
                .send(Certificate(client.client_cert.to_der()?))
                .await?;

            return Ok(xml! {
                <root status_code=200>
                    <paired>1</paired>
                </root>
            }
            .to_string());
        }
    }

    Ok(xml! {
        <root status_code=200>
            <paired>0</paired>
        </root>
    }
    .to_string())
}

#[derive(Deserialize, StateData, StaticResponseExtender)]
pub struct PairingQueryExtractor {
    uniqueid: String,
    phrase: Option<String>,
    salt: Option<String>,
    clientcert: Option<String>,
    clientchallenge: Option<String>,
    serverchallengeresp: Option<String>,
    clientpairingsecret: Option<String>,
}

pub enum PairingVariant {
    GetServerCert { salt: String, clientcert: String },
    ClientChallenge { clientchallenge: String },
    ServerChallengeResp { serverchallengeresp: String },
    ClientPairingSecret { clientpairingsecret: String },
}

impl std::convert::TryFrom<PairingQueryExtractor> for PairingVariant {
    type Error = ();

    fn try_from(fields: PairingQueryExtractor) -> Result<Self, ()> {
        if fields.phrase.is_some()
            && fields.phrase.unwrap() == "getservercert"
            && fields.salt.is_some()
            && fields.clientcert.is_some()
        {
            Ok(PairingVariant::GetServerCert {
                salt: fields.salt.unwrap(),
                clientcert: fields.clientcert.unwrap(),
            })
        } else if fields.clientchallenge.is_some() {
            Ok(PairingVariant::ClientChallenge {
                clientchallenge: fields.clientchallenge.unwrap(),
            })
        } else if fields.serverchallengeresp.is_some() {
            Ok(PairingVariant::ServerChallengeResp {
                serverchallengeresp: fields.serverchallengeresp.unwrap(),
            })
        } else if fields.clientpairingsecret.is_some() {
            Ok(PairingVariant::ClientPairingSecret {
                clientpairingsecret: fields.clientpairingsecret.unwrap(),
            })
        } else {
            Err(())
        }
    }
}

#[allow(non_snake_case)]
#[derive(Deserialize, StateData, StaticResponseExtender)]
pub struct LaunchQueryExtractor {
    uniqueid: String,
    //uuid
    appid: usize,
    mode: String,
    //additionalStates=1
    //sops=0
    rikey: String,
    rikeyid: String,
    //localAudioPlayMode: String,
    //surroundAudioInfo: u64,
    //remoteControllersBitmap: String,
    //gcmap: String,
}
