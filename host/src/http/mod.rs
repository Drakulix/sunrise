use crate::{ClientInfo, SharedState};

use std::{
    future::Future,
    panic::RefUnwindSafe,
    pin::Pin,
    sync::{Arc, Mutex},
    time::SystemTime,
};

use anyhow::{Context, Result};
use gotham::{
    handler::IntoResponse,
    middleware::{logger::RequestLogger, state::StateMiddleware},
    pipeline::{new_pipeline, single_pipeline},
    plain::init_server,
    prelude::{DefineSingleRoute, DrawRoutes},
    router::{build_router, Router},
    rustls::{
        internal::msgs::handshake::DistinguishedNames,
        server::{ClientCertVerified, ClientCertVerifier},
        Certificate, Error as TlsError, PrivateKey, ServerConfig,
    },
    state::StateData,
    tls::init_server as tls_init_server,
    StartError,
};
use openssl::{
    error::ErrorStack,
    stack::Stack,
    x509::{
        store::{X509Store, X509StoreBuilder},
        verify::X509VerifyFlags,
        X509StoreContext, X509,
    },
};
use rustls::{client::HandshakeSignatureValid, internal::msgs::handshake::DigitallySignedStruct};
use tokio::sync::mpsc;

use self::handlers::LaunchQueryExtractor;

mod handlers;

pub struct HttpState {
    pub http_server: Pin<Box<dyn Future<Output = Result<(), StartError>>>>,
    pub https_server: Pin<Box<dyn Future<Output = Result<(), StartError>>>>,
}

#[derive(Clone, StateData)]
struct AddCert {
    add_cert: mpsc::Sender<Certificate>,
}
impl RefUnwindSafe for AddCert {}

fn map_openssl_to_rustls_err(e: ErrorStack) -> TlsError {
    TlsError::General(e.errors().iter().map(|e| format!("{}", e)).fold(
        String::new(),
        |mut str, err| {
            str.push_str(&err);
            str
        },
    ))
}

struct MoonlightVerifier {
    new_certs: Mutex<mpsc::Receiver<Certificate>>,
    client_certs: Mutex<Vec<X509>>,
    store: Mutex<X509Store>,
}

impl MoonlightVerifier {
    pub fn new(recv: mpsc::Receiver<Certificate>) -> Result<MoonlightVerifier, ErrorStack> {
        Ok(MoonlightVerifier {
            new_certs: Mutex::new(recv),
            client_certs: Mutex::new(Vec::new()),
            store: Mutex::new(X509StoreBuilder::new()?.build()),
        })
    }
}

impl ClientCertVerifier for MoonlightVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> Option<bool> {
        Some(true)
    }

    fn client_auth_root_subjects(&self) -> Option<DistinguishedNames> {
        Some(Vec::new())
    }

    fn verify_client_cert(
        &self,
        end_entity: &Certificate,
        intermediates: &[Certificate],
        _now: SystemTime,
    ) -> Result<ClientCertVerified, TlsError> {
        let mut added = false;
        let mut client_certs = self.client_certs.lock().unwrap();
        while let Ok(cert) = self.new_certs.lock().unwrap().try_recv() {
            client_certs.push(X509::from_der(&*cert.0).map_err(map_openssl_to_rustls_err)?);
            added = true;
        }

        let mut store = self.store.lock().unwrap();
        if added {
            let mut new_store = X509StoreBuilder::new().map_err(map_openssl_to_rustls_err)?;
            for cert in client_certs.iter() {
                new_store
                    .add_cert(cert.clone())
                    .map_err(map_openssl_to_rustls_err)?
            }
            new_store
                .set_flags(X509VerifyFlags::PARTIAL_CHAIN)
                .map_err(map_openssl_to_rustls_err)?;
            *store = new_store.build();
        }

        let mut context = X509StoreContext::new().map_err(map_openssl_to_rustls_err)?;
        let cert = X509::from_der(&*end_entity.0).map_err(map_openssl_to_rustls_err)?;
        let cert_chain = {
            let mut stack = Stack::new().map_err(map_openssl_to_rustls_err)?;
            for cert in intermediates {
                stack
                    .push(X509::from_der(&*cert.0).map_err(map_openssl_to_rustls_err)?)
                    .map_err(map_openssl_to_rustls_err)?;
            }
            stack
        };
        let result = context
            .init(&**store, &*cert, &*cert_chain, |context| {
                let mut result = context.verify_cert()?;
                if !result {
                    match context.error().as_raw() {
                        18 => {
                            result = true;
                        } // X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT
                        79 => {
                            result = true;
                        } // X509_V_ERR_INVALID_CA
                        20 => {
                            result = true;
                        } // X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY
                        9 => {
                            result = true;
                        } // X509_V_ERR_CERT_NOT_YET_VALID
                        10 => {
                            result = true;
                        } // X509_V_ERR_CERT_HAS_EXPIRED
                        _ => {}
                    }
                }
                Ok(result)
            })
            .map_err(map_openssl_to_rustls_err)?;

        result
            .then_some(ClientCertVerified::assertion())
            .ok_or(TlsError::InvalidCertificateSignature)
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &Certificate,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &Certificate,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }
}

fn http_router(state: SharedState, send: mpsc::Sender<Certificate>) -> Router {
    let (chain, pipelines) = single_pipeline(
        new_pipeline()
            .add(StateMiddleware::new(state))
            .add(StateMiddleware::new(AddCert { add_cert: send }))
            .add(RequestLogger::new(log::Level::Info))
            .build(),
    );

    build_router(chain, pipelines, |route| {
        route
            .get("/serverinfo")
            .with_query_string_extractor::<ClientInfo>()
            .to_async(|state| async {
                let (state, string) = handlers::server_info(state).await;
                let resp = string.into_response(&state);
                Ok((state, resp))
            });
        route
            .get("/pair")
            .with_query_string_extractor::<handlers::PairingQueryExtractor>()
            .to_async(|state| async {
                let (state, string) = handlers::http_pair(state).await;
                let resp = string.into_response(&state);
                Ok((state, resp))
            });
        route
            .get("/unpair")
            .with_query_string_extractor::<ClientInfo>()
            .to_async(|state| async {
                let (state, string) = handlers::unpair(state).await;
                let resp = string.into_response(&state);
                Ok((state, resp))
            });
    })
}

fn https_router(state: SharedState) -> Router {
    let (chain, pipelines) = single_pipeline(
        new_pipeline()
            .add(StateMiddleware::new(state))
            .add(RequestLogger::new(log::Level::Info))
            .build(),
    );

    build_router(chain, pipelines, |route| {
        route
            .get("/serverinfo")
            .with_query_string_extractor::<ClientInfo>()
            .to_async(|state| async {
                let (state, string) = handlers::server_info(state).await;
                let resp = string.into_response(&state);
                Ok((state, resp))
            });
        route
            .get("/pair")
            .with_query_string_extractor::<ClientInfo>()
            .to_async(|state| async {
                let (state, string) = handlers::https_pair(state).await;
                let resp = string.into_response(&state);
                Ok((state, resp))
            });
        route
            .get("/applist")
            .with_query_string_extractor::<ClientInfo>()
            .to_async(|state| async {
                let (state, string) = handlers::applist(state).await;
                let resp = string.into_response(&state);
                Ok((state, resp))
            });
        /*
        route
            .get("/appasset")
            .with_query_string_extractor::<ClientInfo>()
            .to_async(|state| async {
                let (state, string) = handlers::appasset(state).await;
                let resp = string.into_response(&state);
                Ok((state, resp))
            });
        */
        route
            .get("/launch")
            .with_query_string_extractor::<LaunchQueryExtractor>()
            .to_async(|state| async {
                let (state, string) = handlers::launch(state).await;
                let resp = string.into_response(&state);
                Ok((state, resp))
            });
    })
}

pub async fn init(state: SharedState) -> Result<HttpState> {
    let config = state.0.lock().await;

    let der_cert = config
        .server_cert
        .to_der()
        .context("Failed to convert server cert")?;
    let der_key = config
        .server_key
        .private_key_to_der()
        .context("Failed to convert server key")?;

    let (send, recv) = mpsc::channel(10);
    let verifier = MoonlightVerifier::new(recv)?;
    let ssl_config = ServerConfig::builder()
        .with_safe_defaults()
        .with_client_cert_verifier(Arc::new(verifier))
        .with_single_cert(vec![Certificate(der_cert)], PrivateKey(der_key))?;

    let http_server = Box::pin(init_server(
        ("0.0.0.0", config.http_port),
        http_router(state.clone(), send),
    ));
    let https_server = Box::pin(tls_init_server(
        ("0.0.0.0", config.https_port),
        https_router(state.clone()),
        ssl_config,
    ));

    Ok(HttpState {
        http_server,
        https_server,
    })
}
