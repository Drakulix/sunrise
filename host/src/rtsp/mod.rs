use rtsp_types::{self, Message, Method, ParseError, Request, Response, WriteError};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, Error as IoError},
    net::{TcpListener, TcpStream},
    task,
};
use uuid::Uuid;

use crate::SharedState;

pub async fn init() -> std::io::Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", 48010)).await
}

pub async fn new_client(
    listener: TcpListener,
    mut stream: TcpStream,
    state: SharedState,
    id: Uuid,
) {
    task::spawn(async move {
        let _ = stream.set_nodelay(true);
        let _listener = listener;
        let mut buffer = Vec::new();
        while let Ok(_len) = stream.read_buf(&mut buffer).await {
            let len = match Message::parse(&buffer) {
                Ok((message, len)) => {
                    if let Err(err) = handle_message(message, &mut stream, &state, &id).await {
                        log::error!("Error handling RTSP message: {}", err);
                    }
                    len
                }
                Err(ParseError::Incomplete) => 0,
                Err(ParseError::Error) => {
                    break;
                }
            };
            buffer = buffer.split_off(len);
        }
        log::info!("RTSP connection closed");
    });
}

fn handle_options(request: &Request<&[u8]>) -> Response<Vec<u8>> {
    Response::builder(rtsp_types::Version::V1_0, rtsp_types::StatusCode::Ok)
        .typed_header::<rtsp_types::headers::CSeq>(
            &request
                .typed_header::<rtsp_types::headers::CSeq>()
                .unwrap()
                .unwrap(),
        )
        .build(Vec::new())
}

fn handle_describe(request: &Request<&[u8]>) -> Response<Vec<u8>> {
    let video_params = String::new();
    let audio_params = format!(
        "a=fmtp:97 surround-params={}{}{}{}",
        2, // STEREO for now
        1,
        1,
        "0010"
    );

    let payload = format!("{}\n{}\n", video_params, audio_params);
    Response::builder(rtsp_types::Version::V1_0, rtsp_types::StatusCode::Ok)
        .typed_header::<rtsp_types::headers::CSeq>(
            &request
                .typed_header::<rtsp_types::headers::CSeq>()
                .unwrap()
                .unwrap(),
        )
        .build(payload.into_bytes())
}

fn handle_setup(request: &Request<&[u8]>) -> Response<Vec<u8>> {
    log::warn!("{:?}", request);
    unimplemented!()
}

fn handle_annouce(request: &Request<&[u8]>) -> Response<Vec<u8>> {
    unimplemented!()
}

fn handle_play(request: &Request<&[u8]>) -> Response<Vec<u8>> {
    unimplemented!()
}

async fn handle_message(
    message: Message<&[u8]>,
    stream: &mut TcpStream,
    state: &SharedState,
    id: &Uuid,
) -> Result<(), IoError> {
    log::info!("RTSP message: {:?}", message);

    let resp = match message {
        Message::Request(request) => match request.method() {
            Method::Options => Some(handle_options(&request)),
            Method::Describe => Some(handle_describe(&request)),
            Method::Setup => Some(handle_setup(&request)),
            Method::Announce => Some(handle_annouce(&request)),
            Method::Play => Some(handle_play(&request)),
            x => {
                log::error!("Unknown RTSP method: {:?}", x);
                None
            }
        },
        x => {
            log::warn!("Receive Response?: {:?}", x);
            None
        }
    };

    tokio::time::sleep(Duration::from_millis(500)).await;
    if let Some(resp) = resp {
        let mut out_buf = Vec::new();
        if let Err(WriteError::IoError(err)) = resp.write(&mut out_buf) {
            return Err(err);
        }

        let string = std::str::from_utf8(&out_buf).unwrap();
        log::info!("RTSP answer:\n{}", string);

        stream.write_all(&out_buf).await?;
        stream.flush().await?;
    }

    Ok(())
}
