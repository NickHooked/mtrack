// Copyright (C) 2026 Michael Wilson <mike@mdwn.dev>
// SPDX-License-Identifier: GPL-3.0-only
//! One live deck controller, many bounded, latest-frame output subscribers.
use super::server::WebUiState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use futures_util::StreamExt;
use mtrack_core::{
    config::video::{vj::VjConfig, VideoConfig},
    video::vj::{ControlEvent, Deck, MidiInput},
};
use serde::Deserialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;

use crate::video::vj::EncodedCanvas as EncodedFrame;
type Frames = Arc<Vec<EncodedFrame>>;
pub(crate) struct VjHub {
    frames: watch::Sender<Frames>,
}
impl Default for VjHub {
    fn default() -> Self {
        Self {
            frames: watch::channel(Arc::new(vec![])).0,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Start { config: VideoConfig },
    Control { event: ControlEvent },
    Settings { settings: VjConfig },
    Midi { device: String, data: Vec<u8> },
    Stop,
}
enum WorkerCommand {
    Control(ControlEvent),
    Settings(VjConfig),
    Midi(String, Vec<u8>),
}
struct Session {
    cancel: Arc<AtomicBool>,
    commands: crossbeam_channel::Sender<WorkerCommand>,
    status: Arc<parking_lot::Mutex<serde_json::Value>>,
}
struct WorkerGuard {
    hub: Arc<VjHub>,
    status: Arc<parking_lot::Mutex<serde_json::Value>>,
}
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.hub.frames.send_replace(Arc::new(vec![]));
        if std::thread::panicking() {
            *self.status.lock() = serde_json::json!({"type":"state","running":false,"starting":false,"error":"The VJ render worker stopped unexpectedly"});
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Session {
    fn send(&self, command: WorkerCommand) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|_| "VJ control queue is full or the session has stopped".into())
    }
    fn start(
        config: VideoConfig,
        base: std::path::PathBuf,
        hub: Arc<VjHub>,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<Self, String> {
        // Validate before reserving native resources; actual decode remains off the async executor.
        let mut deck = Deck::new(config.clone())?;
        let (commands, receiver) = crossbeam_channel::bounded(256);
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let status = Arc::new(parking_lot::Mutex::new(
            serde_json::json!({"type":"state","running":false,"starting":true}),
        ));
        let report = status.clone();
        std::thread::Builder::new().name("mtrack-vj".into()).spawn(move||{
            // The permit follows the native worker even if its socket disappears mid-startup.
            let _permit=permit;
            let _guard=WorkerGuard {hub:hub.clone(),status:report.clone()};
            let result=(||->Result<(),String>{
                let mut renderer=crate::video::vj::DeckRenderer::new(config.clone(),&base,stop.clone()).map_err(|e|e.to_string())?;
                let origin=Instant::now();let interval=Duration::from_secs_f64(1.0/config.canvas().frame_rate().min(60.0));
                let mut last_midi=serde_json::Value::Null;let mut frames=0u64;let mut last_error:Option<String>=None;
                while !stop.load(Ordering::Acquire) {
                    let started=Instant::now();let now=origin.elapsed().as_secs_f64();
                    for command in receiver.try_iter().take(256) {
                        let result=match command {
                            WorkerCommand::Control(event)=>deck.control(&event,now),
                            WorkerCommand::Settings(settings)=>deck.update_settings(settings),
                            WorkerCommand::Midi(device,data)=>{if let Some(input)=MidiInput::parse(&data){last_midi=serde_json::json!({"device":device,"input":input,"sequence":frames});deck.midi(&device,&data,now);}Ok(())},
                        };
                        if let Err(error)=result {last_error=Some(error);}
                    }
                    deck.advance(now);
                    let Some(encoded)=renderer.render_encoded(&deck).map_err(|e|e.to_string())? else{break};
                    frames+=1;
                    hub.frames.send_replace(Arc::new(encoded));
                    *report.lock()=serde_json::json!({"type":"state","running":true,"starting":false,"state":deck.state,"fps":frames as f64/origin.elapsed().as_secs_f64().max(0.001),"last_midi":last_midi,"error":last_error});
                    // Interruptible sleep: Stop does not wait an entire low-frame-rate period.
                    let deadline=started+interval;while Instant::now()<deadline&&!stop.load(Ordering::Acquire){std::thread::sleep(deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(5)));}
                } Ok(())
            })();
            hub.frames.send_replace(Arc::new(vec![]));
            *report.lock()=serde_json::json!({"type":"state","running":false,"starting":false,"error":result.err()});
        }).map_err(|e|e.to_string())?;
        Ok(Self {
            cancel,
            commands,
            status,
        })
    }
}

pub(crate) fn router() -> Router<WebUiState> {
    Router::new()
        .route("/api/songs/{name}/vj/ws", get(control_handler))
        .route("/api/video/vj/output/{id}/ws", get(output_handler))
        .route("/video-output/{id}", get(output_page))
}
async fn control_handler(
    ws: WebSocketUpgrade,
    Path(name): Path<String>,
    State(state): State<WebUiState>,
) -> impl IntoResponse {
    ws.max_message_size(2 * 1024 * 1024)
        .on_upgrade(move |socket| control_socket(socket, name, state))
}
async fn send_json(socket: &mut WebSocket, value: serde_json::Value) -> bool {
    tokio::time::timeout(
        Duration::from_secs(2),
        socket.send(Message::Text(value.to_string().into())),
    )
    .await
    .is_ok_and(|r| r.is_ok())
}
async fn control_socket(mut socket: WebSocket, name: String, state: WebUiState) {
    let Some(song) = state.player.get_playlist().get_song(&name) else {
        send_json(
            &mut socket,
            serde_json::json!({"type":"error","error":"Song is not loaded"}),
        )
        .await;
        return;
    };
    let mut session: Option<Session> = None;
    let mut midi = state.player.video_midi_input();
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    let mut frames = state.vj.frames.subscribe();
    loop {
        tokio::select! {
            incoming=socket.next()=>{
                let Some(Ok(message))=incoming else{break};
                let Message::Text(text)=message else{if matches!(message,Message::Close(_)){break;}continue};
                let result:Result<(),String>=match serde_json::from_str::<Command>(&text){
                    Err(error)=>Err(format!("Invalid VJ command: {error}")),
                    Ok(Command::Start{config})=>{
                        if session.as_ref().is_some_and(|s|{ let status = s.status.lock(); status["running"]==true||status["starting"]==true }) {Err("Stop the current VJ session before starting another".into())}
                        else if state.player.is_playing().await {Err("Stop song playback before starting a VJ session".into())}
                        else {session.take();match state.video_preview_permit.clone().try_acquire_owned(){
                            Err(_)=>Err("Video is busy. Close the editor preview or wait for the previous session to stop".into()),
                            Ok(permit)=>Session::start(config,song.base_path().to_path_buf(),state.vj.clone(),permit).map(|s|session=Some(s)),
                        }}
                    },
                    Ok(Command::Stop)=>{session.take();Ok(())},
                    Ok(Command::Control{event})=>session.as_ref().ok_or_else(||"Start VJ output first".to_string()).and_then(|s|s.send(WorkerCommand::Control(event))),
                    Ok(Command::Settings{settings})=>session.as_ref().ok_or_else(||"Start VJ output first".to_string()).and_then(|s|s.send(WorkerCommand::Settings(settings))),
                    Ok(Command::Midi{device,data})=>{if device.len()>256||MidiInput::parse(&data).is_none(){Err("Invalid MIDI input".into())}else{session.as_ref().map_or(Ok(()),|s|s.send(WorkerCommand::Midi(device,data)))}}
                };
                if let Err(error)=result {if !send_json(&mut socket,serde_json::json!({"type":"error","error":error})).await{break;}}
            },
            raw=midi.recv()=>{
                match raw {Ok(data)=>{if let Some(s)=&session {let _=s.send(WorkerCommand::Midi("host".into(),data.clone()));}
                    if !send_json(&mut socket,serde_json::json!({"type":"midi","device":"host","input":MidiInput::parse(&data)})).await{break;}
                },Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{},Err(_)=>break}
            },
            _=ticker.tick()=>{
                if session.is_some() && state.player.is_playing().await {session.take();send_json(&mut socket,serde_json::json!({"type":"error","error":"VJ stopped because song playback started"})).await;}
                let value=session.as_ref().map(|s|s.status.lock().clone()).unwrap_or_else(||serde_json::json!({"type":"state","running":false,"starting":false}));
                if !send_json(&mut socket,value).await{break;}
            },
            changed=frames.changed()=>{
                if changed.is_err(){break;}
                let latest=frames.borrow_and_update().clone();
                if session.is_some(){if let Some(frame)=latest.iter().find(|f|f.id=="composition"){
                    if !tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Binary(frame.jpeg.clone().into()))).await.is_ok_and(|r|r.is_ok()){break;}
                }}
            }
        }
    }
}
async fn output_handler(
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    State(state): State<WebUiState>,
) -> impl IntoResponse {
    ws.max_message_size(1024).on_upgrade(move|mut socket|async move{
        let mut frames=state.vj.frames.subscribe();
        loop {
            let latest=frames.borrow_and_update().clone();
            if let Some(frame)=latest.iter().find(|f|f.id==id) {
                if !tokio::time::timeout(Duration::from_secs(2),socket.send(Message::Binary(frame.jpeg.clone().into()))).await.is_ok_and(|r|r.is_ok()){break;}
            } else if !send_json(&mut socket,serde_json::json!({"type":"clear"})).await{break;}
            tokio::select! {change=frames.changed()=>{if change.is_err(){break;}},message=socket.next()=>{if !matches!(message,Some(Ok(Message::Ping(_)))){break;}}}
        }
    })
}
async fn output_page() -> Html<&'static str> {
    Html(include_str!("video_output.html"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrack_core::config::video::vj::ControlTarget;
    fn config() -> VideoConfig {
        serde_json::from_value(serde_json::json!({
            "canvas":{"width":32,"height":24,"frame_rate":20,"background":"#400000"},
            "tracks":[{"name":"Camera","clips":[{"id":"camera","start":0,"duration":10,"source":{"type":"test_pattern"}}]},
                      {"name":"Empty layer","clips":[{"id":"other","start":0,"duration":10,"source":{"type":"test_pattern"}}]}],
            "vj":{"outputs":[
                {"id":"screen","name":"Screen","width":24,"height":16,"slices":[{"id":"s","name":"All"}]},
                {"id":"layer","name":"Layer","width":16,"height":12,"slices":[{"id":"s","name":"Layer 2","source":{"type":"layer","index":1}}]}
            ]}
        })).unwrap()
    }
    async fn eventually(mut check: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(12), async {
            while !check() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("VJ did not reach expected state");
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn vj_worker_routes_outputs_controls_freeze_and_releases_capture_permit() {
        let base = tempfile::tempdir().unwrap();
        let permit = Arc::new(tokio::sync::Semaphore::new(1));
        let hub = Arc::new(VjHub::default());
        let mut frames = hub.frames.subscribe();
        let session = Session::start(
            config(),
            base.path().into(),
            hub.clone(),
            permit.clone().try_acquire_owned().unwrap(),
        )
        .unwrap();
        eventually(|| session.status.lock()["running"] == true).await;
        session
            .send(WorkerCommand::Control(ControlEvent {
                target: ControlTarget::Launch {
                    layer: 0,
                    column: 0,
                },
                value: 1.0,
            }))
            .unwrap();
        eventually(|| session.status.lock()["state"]["layers"][0]["active"]["id"] == "camera")
            .await;
        let latest = frames.borrow_and_update().clone();
        assert_eq!(latest.len(), 3);
        let screen =
            image::load_from_memory(&latest.iter().find(|f| f.id == "screen").unwrap().jpeg)
                .unwrap()
                .to_rgb8();
        assert_eq!(screen.dimensions(), (24, 16));
        assert!(screen.pixels().any(|p| p[1] > 40));
        let isolated =
            image::load_from_memory(&latest.iter().find(|f| f.id == "layer").unwrap().jpeg)
                .unwrap()
                .to_rgb8();
        assert_eq!(isolated.dimensions(), (16, 12));
        assert!(isolated.pixels().all(|p| p.0.iter().all(|v| *v < 5)));
        session
            .send(WorkerCommand::Control(ControlEvent {
                target: ControlTarget::Freeze,
                value: 1.0,
            }))
            .unwrap();
        eventually(|| session.status.lock()["state"]["frozen"] == true).await;
        let frozen = frames.borrow_and_update().clone();
        tokio::time::sleep(Duration::from_millis(160)).await;
        let later = frames.borrow_and_update().clone();
        assert_eq!(frozen[0].jpeg, later[0].jpeg);
        session
            .send(WorkerCommand::Control(ControlEvent {
                target: ControlTarget::Blackout,
                value: 1.0,
            }))
            .unwrap();
        eventually(|| session.status.lock()["state"]["blackout"] == true).await;
        for frame in frames.borrow_and_update().iter() {
            let rgb = image::load_from_memory(&frame.jpeg).unwrap().to_rgb8();
            assert!(rgb.pixels().all(|p| p.0.iter().all(|v| *v < 5)));
        }
        drop(session);
        eventually(|| permit.available_permits() == 1).await;
        assert!(hub.frames.borrow().is_empty());
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn vj_failed_start_releases_the_permit_and_reports_media_failure() {
        let base = tempfile::tempdir().unwrap();
        let permit = Arc::new(tokio::sync::Semaphore::new(1));
        let hub = Arc::new(VjHub::default());
        let mut value = serde_json::to_value(config()).unwrap();
        value["tracks"][0]["clips"][0]
            .as_object_mut()
            .unwrap()
            .remove("source");
        value["tracks"][0]["clips"][0]["file"] = serde_json::json!("missing.ogv");
        let session = Session::start(
            serde_json::from_value(value).unwrap(),
            base.path().into(),
            hub,
            permit.clone().try_acquire_owned().unwrap(),
        )
        .unwrap();
        eventually(|| permit.available_permits() == 1).await;
        assert!(session.status.lock()["error"]
            .as_str()
            .unwrap()
            .contains("missing.ogv"));
    }
    #[test]
    fn vj_commands_reject_unexpected_fields_and_injected_paths() {
        assert!(serde_json::from_str::<Command>(
            r#"{"type":"control","event":{"target":{"type":"blackout"},"value":1},"shell":"bad"}"#
        )
        .is_err());
        let mut value = serde_json::to_value(config()).unwrap();
        value["tracks"][0]["clips"][0]
            .as_object_mut()
            .unwrap()
            .remove("source");
        value["tracks"][0]["clips"][0]["file"] = serde_json::json!("../outside.ogv");
        assert!(Deck::new(serde_json::from_value(value).unwrap()).is_err());
    }
}
