//! `GET /api/v1/displays/{id}/stream`: a WebSocket whose binary messages are frames.
//!
//! The stream owns the display until other content is set. Frames may arrive at any rate;
//! the display shows the newest one it can. Errors are reported as text messages with an
//! [`ErrorBody`] and do not close the stream.

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use ssp_core::Frame;

use super::types::{ErrorBody, StreamFormat, StreamQuery};
use super::{ApiError, AppState, blocking};
use crate::manager::StreamTicket;

/// Largest frame message accepted (raw RGBA for big panels).
const MAX_MESSAGE_BYTES: usize = 64 << 20;

/// Close code sent when other content replaced the stream (application-defined range).
const CLOSE_REPLACED: u16 = 4000;

pub async fn stream(
    ws: WebSocketUpgrade,
    State(app): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StreamQuery>,
) -> Result<Response, ApiError> {
    let manager = app.manager.clone();
    let ticket = blocking(move || Ok(manager.begin_stream(&id)?)).await?;
    tracing::info!(display = %ticket.id, "stream started");
    Ok(ws
        .max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, app, ticket, query)))
}

async fn run(mut socket: WebSocket, app: AppState, ticket: StreamTicket, query: StreamQuery) {
    let mut shutdown = app.shutdown.clone();
    let fit = query.fit.unwrap_or_default().into();
    loop {
        let message = tokio::select! {
            message = socket.recv() => message,
            _ = shutdown.changed() => break,
        };
        let data = match message {
            Some(Ok(Message::Binary(data))) => data,
            Some(Ok(Message::Close(_)) | Err(_)) | None => break,
            Some(Ok(_)) => continue,
        };
        if !ticket.is_current() {
            let close = CloseFrame {
                code: CLOSE_REPLACED,
                reason: "replaced by other content".into(),
            };
            let _ = socket.send(Message::Close(Some(close))).await;
            break;
        }
        let manager = app.manager.clone();
        let id = ticket.id.clone();
        let result = blocking(move || {
            // Looked up per frame so the stream survives the display reconnecting.
            let device = manager.device(&id)?;
            let panel = device.presenter.info().panel;
            let (width, height) = (panel.width, panel.height);
            let frame = match query.format {
                StreamFormat::Image => Frame::decode(&data, width, height, fit),
                StreamFormat::Rgb => Frame::from_rgb(width, height, data.to_vec()),
                StreamFormat::Rgba => Frame::from_rgba(width, height, &data),
            }?;
            device.submit(frame)?;
            Ok(())
        })
        .await;
        if let Err(err) = result {
            let body = serde_json::to_string(&ErrorBody { error: err.message }).unwrap_or_default();
            if socket.send(Message::Text(body.into())).await.is_err() {
                break;
            }
        }
    }
    tracing::info!(display = %ticket.id, "stream ended");
}
