//!
//! This package is the central server that controls all the rooms.
//! Every client websocket should send us an event with each action they take.
//!
//! This makes the code quite simple as it doesn't need to comply with any async strangeness but
//! it creates a bottleneck as every single event in all of the server passes trough a single thread.
//! (stress performance test needed). In addition the delay between packet sharing between threads
//! adds up.
//!
//! Additional work is being done to decentralize this, replacing it with a
//!

use actix::prelude::*;
use actix::dev::{MessageResponse, ResponseChannel};
use rand::{self, rngs::ThreadRng, Rng};
use std::collections::{HashMap, HashSet};

use crate::protocol::{OutEvent, IdType, PlayerObject, RoomConnectionType, SerId, LoginData, PlayerCosmetics};
use crate::client_ws::ClientWs;


// Copied from actix, love the library but it seems a bit rushed in the "actor" part.
// This should generate the code to share a result between actors.
macro_rules! simple_result {
    ($type:ty) => {
        impl<A, M> MessageResponse<A, M> for $type
        where
            A: Actor,
            M: Message<Result = $type>,
        {
            fn handle<R: ResponseChannel<M>>(self, _: &mut A::Context, tx: Option<R>) {
                if let Some(tx) = tx {
                    tx.send(self);
                }
            }
        }
    };
}


#[derive(Message)]
#[rtype(result = "()")]
pub struct Event(pub OutEvent);

#[derive(Message)]
#[rtype(IdType)]
pub struct RegisterSession {
    pub id: Option<IdType>,
    pub addr: Addr<ClientWs>,
    pub obj: LoginData,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Disconnect {
    pub id: IdType,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct EditCosmetics {
    pub id: IdType,
    pub obj: PlayerCosmetics,
}

#[derive(Message)]
#[rtype(CreateRoomResult)]
pub struct CreateRoom {
    pub id: IdType,
}

pub struct CreateRoomResult {
    pub room_id: IdType,
    pub player: PlayerObject,
}

simple_result!(CreateRoomResult);

#[derive(Message)]
#[rtype(JoinRoomResult)]
pub struct JoinRoom {
    pub id: IdType,
    pub room_id: IdType,
}

pub enum JoinRoomResult {
    Success(Vec<PlayerObject>),
    RoomNotFound,
    NameConflict,
    AlreadyPlaying,
}
simple_result!(JoinRoomResult);

#[derive(Message)]
#[rtype(result = "()")]
pub struct LeaveRoom {
    pub id: IdType,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct StartRoom {
    pub id: IdType,
    pub conn_type: RoomConnectionType,
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct SendRelayMex {
    pub sender_id: IdType,
    pub data: String,
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct SendRelayMexRaw {
    pub data: String,
}


struct UserData {
    addr: Addr<ClientWs>,
    obj: PlayerObject,
    room: Option<IdType>,
}

struct RoomData {
    state: RoomState,
    players: HashSet<IdType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomState {
    Matchmaking,
    Playing
}


pub struct ServerActor {
    players: HashMap<IdType, UserData>,
    rooms: HashMap<IdType, RoomData>,
    rng: ThreadRng,
}

impl Default for ServerActor {
    fn default() -> Self {
        ServerActor {
            players: HashMap::new(),
            rooms: HashMap::new(),
            rng: rand::thread_rng(),
        }
    }
}

impl Actor for ServerActor {
    /// We are going to use simple Context, we just need ability to communicate
    /// with other actors.
    type Context = Context<Self>;
}

impl ServerActor {
    fn allocate_player_id(&mut self, mut data: UserData) -> IdType {
        let mut id;

        loop {
            id = self.rng.gen::<IdType>();

            if !self.players.contains_key(&id) {
                break;
            }
        }
        data.obj.id = id.into();
        self.players.insert(id, data);
        id
    }

    fn allocate_room_id(&mut self, host: IdType) -> IdType {
        let mut id;

        loop {
            id = self.rng.gen::<IdType>();

            if !self.rooms.contains_key(&id) {
                break;
            }
        }

        let mut set = HashSet::new();
        set.insert(host);
        let room = RoomData {
            state: RoomState::Matchmaking,
            players: set,
        };
        self.rooms.insert(id, room);

        id
    }

    /// Send event to all users in the room
    fn broadcast_event(&self, room: IdType, event: OutEvent, skip_id: Option<IdType>) {
        if let Some(room) = self.rooms.get(&room) {
            for id in room.players.iter() {
                if Some(*id) == skip_id {
                    continue;
                }
                if let Some(x) = self.players.get(&id) {
                    let _ = x.addr.do_send(Event(event.clone()));// TODO: remove clone
                }
            }
        }
    }

    fn leave_room_if_any(&mut self, player_id: IdType) {
        let player = match self.players.get_mut(&player_id) {
            Some(x) => x,
            None => return,
        };
        let room_id = match player.room {
            Some(x) => x,
            None => return,
        };

        let room = self.rooms.get_mut(&room_id).expect("Cannot find room");
        room.players.remove(&player_id);

        let was_player_host = player.obj.is_host;
        player.room = None;
        player.obj.is_host = false;

        if let Some(first_player) = room.players.iter().next() {
            let new_host = if was_player_host {
                let mut p = self.players.get_mut(first_player).expect("Invalid player");
                p.obj.is_host = true;
                Some(p.obj.id)
            } else {
                None
            };

            let event = OutEvent::EventPlayerLeft {
                player: player_id.into(),
                new_host,
            };

            self.broadcast_event(room_id, event, None);
        } else {
            self.rooms.remove(&room_id);
        }
    }
}

impl Handler<RegisterSession> for ServerActor {
    type Result = IdType;

    fn handle(&mut self, msg: RegisterSession, _: &mut Context<Self>) -> Self::Result {
        match msg.id {
            Some(id) => {
                let player = self.players.get_mut(&id).expect("Invalid player");
                if player.room.is_none() {
                    player.obj.username = msg.obj.username;
                    player.obj.cosmetics = msg.obj.cosmetics;
                }
                id
            },
            None => {
                let pobj = PlayerObject {
                    id: 0.into(),
                    username: msg.obj.username,
                    cosmetics: msg.obj.cosmetics,
                    is_host: false
                };
                self.allocate_player_id(UserData {
                    addr: msg.addr,
                    obj: pobj,
                    room: None,
                })
            }
        }

    }
}

impl Handler<Disconnect> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: Disconnect, _: &mut Context<Self>) -> Self::Result {
        self.leave_room_if_any(msg.id);
        self.players.remove(&msg.id);
    }
}

impl Handler<CreateRoom> for ServerActor {
    type Result = CreateRoomResult;

    fn handle(&mut self, msg: CreateRoom, _: &mut Context<Self>) -> Self::Result {
        self.leave_room_if_any(msg.id);
        let room_id = self.allocate_room_id(msg.id);
        let player = self.players.get_mut(&msg.id).expect("Cannot find player");
        player.room = Some(room_id);
        player.obj.is_host = true;
        CreateRoomResult {
            room_id,
            player: player.obj.clone()
        }
    }
}

impl Handler<JoinRoom> for ServerActor {
    type Result = JoinRoomResult;

    fn handle(&mut self, msg: JoinRoom, _: &mut Context<Self>) -> Self::Result {
        self.leave_room_if_any(msg.id);

        let room = match self.rooms.get(&msg.room_id) {
            Some(x) => x,
            None => return JoinRoomResult::RoomNotFound,
        };

        if room.state != RoomState::Matchmaking {
            return JoinRoomResult::AlreadyPlaying;
        }

        let username = self.players.get(&msg.id).expect("Cannot find player").obj.username.as_str();

        let name_conflict = room.players.iter().any(|x| {
            self.players.get(x).expect("Cannot find player").obj.username == username
        });

        if name_conflict {
            return JoinRoomResult::NameConflict;
        }
        let obj = self.players.get_mut(&msg.id).expect("Cannot find player");

        let obj_data = obj.obj.clone();
        obj.room = Some(msg.room_id);
        self.broadcast_event(msg.room_id, OutEvent::EventPlayerJoined {
            player: obj_data
        }, None);

        self.rooms.get_mut(&msg.room_id).expect("Cannot find room").players.insert(msg.id);

        let users = self.rooms.get(&msg.room_id)
            .unwrap()
            .players
            .iter()
            .map(|x| self.players.get(x).expect("Cannot find player").obj.clone())
            .collect();
        JoinRoomResult::Success(users)
    }
}

impl Handler<EditCosmetics> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: EditCosmetics, _: &mut Context<Self>) -> Self::Result {
        let player = self.players.get_mut(&msg.id).expect("Invalid player");

        if player.obj.cosmetics == msg.obj {
            return;
        }
        player.obj.cosmetics = msg.obj.clone();

        let room = match player.room {
            Some(x) => x,
            None => return,
        };

        let id = player.obj.id;

        self.broadcast_event(room, OutEvent::EventPlayerAvatarChange {
            player: id.into(),
            cosmetics: msg.obj,
        }, Some(msg.id));
    }
}

impl Handler<LeaveRoom> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: LeaveRoom, _: &mut Context<Self>) -> Self::Result {
        self.leave_room_if_any(msg.id);
    }
}

impl Handler<StartRoom> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: StartRoom, _: &mut Context<Self>) -> Self::Result {
        let room_id = match self.players.get(&msg.id).and_then(|x| x.room) {
            Some(x) => x,
            None => return,
        };

        if let Some(room) = self.rooms.get_mut(&room_id) {
            if room.state != RoomState::Matchmaking || room.players.len() < 2 {
                return
            }

            room.state = RoomState::Playing;

            let event = OutEvent::EventRoomStart {
                connection_type: msg.conn_type,
                broadcast_id: format!("{}", room_id)
            };

            for id in room.players.iter() {
                if let Some(x) = self.players.get(&id) {
                    let _ = x.addr.do_send(Event(event.clone()));// TODO: remove clone
                }
            }
        }
    }
}

impl Handler<SendRelayMex> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: SendRelayMex, _ctx: &mut Context<Self>) -> Self::Result {
        // TODO: do not clone.
        // it's better to create a queue with multiple indexes
        // A B C D E
        //^     ^   ^
        //p1    p2  p4
        //      p3
        if msg.data.is_empty() {
            return;
        }

        let player = self.players.get(&msg.sender_id).expect("Expected player");
        if let Some(room) = player.room.and_then(|room| self.rooms.get(&room)) {
            let raw = format!("{{\"sender\":\"{}\",{}", SerId(msg.sender_id), &msg.data[1..]);
            let raw_pkt = SendRelayMexRaw { data: raw };
            for player in room.players.iter() {
                if *player == msg.sender_id {
                    continue;
                }
                if let Some(player) = self.players.get(&player) {
                    player.addr.do_send(raw_pkt.clone())
                }
            }
        }
    }
}
