use std::time::{Duration, Instant};

use actix::{Actor, Addr, AsyncContext, prelude::*, StreamHandler};
use actix_web::{Error, HttpRequest, HttpResponse, web};
use actix_web_actors::ws;
use serde::Serialize;

use crate::protocol::{IdMessage, IdType, LoginResponse, NoData, OutEvent, OutMessage, ReceivedMessage, Response, RoomCreateResponse, RoomJoinResponse};
use crate::protocol;
use crate::server_actor::{self, Event, JoinRoomResult, SendRelayMexRaw, ServerActor};

/// How often heartbeat pings are sent
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long before lack of client response causes a timeout
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

const RELAY_QUEUE_MAX_SIZE: usize = 64usize;

pub enum ClientState {
    PreLogin,// What's your name sir?
    MatchMaking,// Join or Create room (can also re-login to change name)
    Lobby,// You're in a room, prepare for battle (can also change cosmetics).
    PrePlaying(u64),// The game is started but the client hasn't acknowledged it yet.
    Playing// Playing.
}

pub struct ClientWs {
    state: ClientState,
    last_hb: Instant,
    session_id: IdType,
    next_send_id: u64,
    db: Addr<ServerActor>,
    relay_queue: Vec<server_actor::SendRelayMexRaw>,
}

impl ClientWs {
    pub fn new(db: Addr<ServerActor>) -> Self {
        ClientWs {
            state: ClientState::PreLogin,
            last_hb: Instant::now(),
            session_id: 0,
            next_send_id: 0,
            db,
            relay_queue: Vec::new(),
        }
    }

    /// helper method that sends ping to client every second.
    ///
    /// also this method checks heartbeats from client
    fn start_heartbeat_checker(&self, ctx: &mut ws::WebsocketContext<Self>) {
        ctx.run_interval(HEARTBEAT_INTERVAL, |act, ctx| {
            // check client heartbeats
            if Instant::now().duration_since(act.last_hb) > CLIENT_TIMEOUT {
                // heartbeat timed out
                println!("Websocket Client heartbeat failed, disconnecting!");

                // stop actor
                ctx.stop();

                // don't try to send a ping
                return;
            }

            ctx.ping(b"");
        });
    }
}

impl Actor for ClientWs {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        self.start_heartbeat_checker(ctx)
    }

    fn stopping(&mut self, _: &mut Self::Context) -> Running {
        match self.state {
            ClientState::PreLogin => {},
            _ => {
                self.db.do_send(server_actor::Disconnect {
                    id: self.session_id,
                });
            },
        }
        Running::Stop
    }
}

impl ClientWs {
    pub fn allocate_id(&mut self) -> u64 {
        let id = self.next_send_id;
        self.next_send_id += 1;
        id.into()
    }

    pub fn send_message<T: ?Sized + Serialize> (&mut self, ctx: &mut <Self as Actor>::Context, inner: &T) -> u64 {
        let id = self.allocate_id();
        let mex = OutMessage {
            id, mex: inner
        };

        let text = serde_json::to_string(&mex).expect("Error serializing message");
        ctx.text(text);
        id
    }

    pub fn handle_message_login(&mut self, ctx: &mut <Self as Actor>::Context, id: u64, mex: ReceivedMessage) {
        if let ReceivedMessage::Login { details } = mex {
            self.db.send(server_actor::RegisterSession {
                id: None,
                addr: ctx.address(),
                obj: details,
            })
                .into_actor(self)
                .then(move |res, act, ctx| {
                    match res {
                        Ok(res) => act.session_id = res,
                        _ => {
                            // something is wrong with chat server
                            ctx.stop();
                            return fut::ready(());
                        },
                    }
                    let res = Response::ok(
                        id, "login_response".into(),
                        LoginResponse {
                            player_id: act.session_id.into(),
                        }
                    );
                    act.state = ClientState::MatchMaking;
                    act.send_message(ctx, &res);
                    fut::ready(())
                })
                .wait(ctx);
        } else {
            self.send_message(ctx, &protocol::Error::from_origin(id, "Login Required".into(), None));
        }
    }

    pub fn handle_message_matchmaking(&mut self, ctx: &mut <Self as Actor>::Context, id: u64, mex: ReceivedMessage) {
        match mex {
            ReceivedMessage::Login { details } => {
                self.db.send(server_actor::RegisterSession {
                    id: Some(self.session_id),
                    addr: ctx.address(),
                    obj: details,
                })
                    .into_actor(self)
                    .then(move |res, act, ctx| {
                        match res {
                            Ok(_) => {},
                            _ => {
                                // something is wrong with chat server
                                ctx.stop();
                                return fut::ready(());
                            },
                        }
                        let res = Response::ok(
                            id, "login_response".into(),
                            LoginResponse {
                                player_id: act.session_id.into(),
                            }
                        );
                        act.send_message(ctx, &res);
                        fut::ready(())
                    })
                    .wait(ctx);
            }
            ReceivedMessage::RoomCreate {} => {
                self.db.send(server_actor::CreateRoom {
                    id: self.session_id
                })
                    .into_actor(self)
                    .then(move |res, act, ctx| {
                        let res = match res {
                            Ok(res) => res,
                            _ => {
                                // something is wrong with chat server
                                ctx.stop();
                                return fut::ready(());
                            },
                        };
                        let pkt = Response::ok(
                            id, "room_create_response".into(),
                            RoomCreateResponse {
                                players: [res.player],
                                invite_id: res.room_id.into(),
                            }
                        );
                        act.send_message(ctx, &pkt);
                        act.state = ClientState::Lobby;

                        fut::ready(())
                    }).wait(ctx);
            },
            ReceivedMessage::RoomJoin { invite_id } => {
                self.db.send(server_actor::JoinRoom {
                    id: self.session_id,
                    room_id: invite_id.into(),
                })
                    .into_actor(self)
                    .then(move |res, act, ctx| {
                        let res = match res {
                            Ok(res) => res,
                            _ => {
                                // something is wrong with chat server
                                ctx.stop();
                                return fut::ready(());
                            },
                        };
                        let ptype = "room_join_response".into();
                        match res {
                            JoinRoomResult::Success(players) => {
                                let pkt = Response::ok(
                                    id, ptype,
                                    RoomJoinResponse { players }
                                );
                                act.send_message(ctx, &pkt);
                                act.state = ClientState::Lobby;
                            }
                            JoinRoomResult::RoomNotFound => {
                                let pkt = Response::from(
                                    id, ptype, Some("room_not_found".into()), NoData {}
                                );
                                act.send_message(ctx, &pkt);
                            },
                            JoinRoomResult::NameConflict => {
                                let pkt = Response::from(
                                    id, ptype, Some("name_conflict".into()), NoData {}
                                );
                                act.send_message(ctx, &pkt);
                            },
                            JoinRoomResult::AlreadyPlaying => {
                                let pkt = Response::from(
                                    id, ptype, Some("already_playing".into()), NoData {}
                                );
                                act.send_message(ctx, &pkt);
                            },
                        }
                        fut::ready(())
                    })
                    .wait(ctx);
            },
            _ => {
                self.send_message(ctx, &protocol::Error::from_origin(id, "Invalid message type".into(), None));
            }
        }
    }

    pub fn handle_message_lobby(&mut self, ctx: &mut <Self as Actor>::Context, id: u64, mex: ReceivedMessage) {
        self.send_message(ctx, &protocol::Error::from_origin(id, "Login Required".into(), None));

        match mex {
            ReceivedMessage::ChangeAvatar { cosmetics } => {
                self.db.do_send(server_actor::EditCosmetics {
                    id: self.session_id,
                    obj: cosmetics,
                })
            },
            ReceivedMessage::RoomLeave {} => {
                self.db.do_send(server_actor::LeaveRoom {
                    id: self.session_id
                });
                self.state = ClientState::MatchMaking;
                self.send_message(ctx, &Response::ok(id, "room_leave_response".into(), NoData {}));
            },
            ReceivedMessage::RoomStart { connection_type } => {
                self.db.do_send(server_actor::StartRoom {
                    id: self.session_id,
                    conn_type: connection_type
                });
            },
            ReceivedMessage::EventRoomStartAck { request_id } => {
                if let ClientState::PrePlaying(res_id) = &self.state {
                    if *res_id == request_id {
                        self.state = ClientState::Playing;
                        for x in self.relay_queue.drain(..) {
                            ctx.text(x.data);
                        }
                    } else {
                        self.send_message(ctx, &protocol::Error::from_origin(id, "Invalid request_id".into(), None));
                    }
                } else {
                    self.send_message(ctx, &protocol::Error::from_origin(id, "Invalid state".into(), Some("No message to acknowledge".into())));
                }
            },
            _ => {
                self.send_message(ctx, &protocol::Error::from_origin(id, "Invalid message type".into(), None));
            }
        }
    }

    pub fn handle_message(&mut self, ctx: &mut <Self as Actor>::Context, id: u64, mex: ReceivedMessage) {
        match &self.state {
            ClientState::PreLogin => {
                self.handle_message_login(ctx, id, mex);
            },
            ClientState::MatchMaking => {
                self.handle_message_matchmaking(ctx, id, mex);
            },
            ClientState::Lobby | ClientState::PrePlaying(..) => {
                self.handle_message_lobby(ctx, id, mex);
            },
            ClientState::Playing => {},
        }
    }
}

impl Handler<server_actor::Event> for ClientWs {
    type Result = ();

    fn handle(&mut self, msg: Event, ctx: &mut <Self as Actor>::Context) -> Self::Result {
        let id = self.send_message(ctx, &msg.0);

        if let OutEvent::EventRoomStart { .. } = msg.0 {
            self.state = ClientState::PrePlaying(id);
        }
    }
}

impl Handler<server_actor::SendRelayMexRaw> for ClientWs {
    type Result = ();

    fn handle(&mut self, msg: SendRelayMexRaw, ctx: &mut <Self as Actor>::Context) -> Self::Result {
        match &mut self.state {
            ClientState::PreLogin => {},
            ClientState::MatchMaking => {},
            ClientState::Lobby => {},
            ClientState::PrePlaying(_) => {
                if self.relay_queue.len() >= RELAY_QUEUE_MAX_SIZE {
                    eprintln!("Client {} not responding to event_room_start, queue full. kicking out", self.session_id);
                    ctx.stop();
                    return;
                }
                self.relay_queue.push(msg)
            },
            ClientState::Playing => {
                ctx.text(msg.data);
            },
        }
    }
}

/// Handler for ws::Message message
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for ClientWs {
    fn handle(
        &mut self,
        msg: Result<ws::Message, ws::ProtocolError>,
        ctx: &mut Self::Context,
    ) {
        let msg = match msg {
            Ok(x) => x,
            Err(_) => {
                ctx.stop();
                return;
            }
        };

        let text = match msg {
            ws::Message::Ping(msg) => {
                self.last_hb = Instant::now();
                ctx.pong(&msg);
                return
            },
            ws::Message::Pong(_) => {
                self.last_hb = Instant::now();
                return
            }
            ws::Message::Text(text) => text,
            ws::Message::Close(_) => {
                ctx.stop();
                return
            },
            _ => return,
        };

        match self.state {
            ClientState::Playing => {
                self.db.do_send(server_actor::SendRelayMex {
                    sender_id: self.session_id,
                    data: text
                });
                return;
            },
            _ => {}
        }

        let id_message = match serde_json::from_str::<IdMessage>(&text) {
            Ok(x) => x,
            Err(_) => {
                let err = protocol::Error::from("Invalid Json".into(), None);
                self.send_message(ctx, &err);
                return
            },
        };

        let id = match id_message.id {
            None => {
                let err = protocol::Error::from("Id missing".into(), None);
                self.send_message(ctx, &err);
                return
            },
            Some(x) => x,
        };

        let mex = match serde_json::from_str::<ReceivedMessage>(&text) {
            Ok(x) => x,
            Err(x) => {
                let err = protocol::Error::from_origin(id, "Invalid Json".into(), Some(x.to_string().into()));
                self.send_message(ctx, &err);
                return;
            }
        };

        self.handle_message(ctx, id, mex);
    }
}

pub async fn matchmaking_start(
    req: HttpRequest,
    stream: web::Payload,
    data: web::Data<Addr<server_actor::ServerActor>>,
) -> Result<HttpResponse, Error> {
    ws::start(ClientWs::new(data.get_ref().clone()), &req, stream)
}
