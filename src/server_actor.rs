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

use std::{cell::RefCell, collections::{HashMap, HashSet}, iter::Successors, ops::DerefMut, time::Duration};

use actix::dev::{MessageResponse, ResponseChannel};
use actix::prelude::*;
use rand::{self, Rng, rngs::ThreadRng};

use crate::client_ws::ClientWs;
use crate::protocol::{IdType, LoginData, OutEvent, OutGameEvent, PlayerCosmetics, PlayerObject, RoomConnectionType, SerId};

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

const MAX_PLAYERS_PER_ROOM: usize = 8;
const MIN_PLAYERS_PER_ROOM: usize = 3;
const ROOM_COUNTDOWN_ON_MIN_PLAYERS: u64 = 30;

#[derive(Message)]
#[rtype(result = "()")]
pub struct Event(pub OutEvent);

#[derive(Message)]
#[rtype(result = "()")]
pub struct GameEvent(pub OutGameEvent);

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

// ----------------------------------------------------------------

#[derive(Message)]
#[rtype(FindRoomResult)]
pub struct FindRoom {
    pub id: IdType
}

pub enum FindRoomResult {
    Success {
        room_id: IdType, 
        players: Vec<PlayerObject>,
        just_created: bool
    }, 
    GameIsFull,
}

simple_result!(FindRoomResult);

// ----------------------------------------------------------------

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
    RoomIsFull,
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

#[derive(Message, Clone)]
#[rtype(result = "Option<GameEndAck>")]
pub struct GameEndRequest {
    pub id: IdType,
}

pub struct GameEndAck(pub Vec<PlayerObject>);
simple_result!(GameEndAck);


struct UserData {
    addr: Addr<ClientWs>,
    obj: PlayerObject,
    room: Option<IdType>,
    in_game: bool,
}

struct RoomData {
    state: RoomState,
    players: HashSet<IdType>,
    in_game_count: u32,

    start_countdown_handle: Option<SpawnHandle>
}

impl RoomData {
    pub fn cancel_start_countdown(&mut self, ctx: &mut Context<ServerActor>) -> bool {
        if let Some(handle) = self.start_countdown_handle {
            ctx.cancel_future(handle);
            self.start_countdown_handle = None;
            return true;
        }
        return false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomState {
    Matchmaking,
    Playing
}


pub struct ServerActor {
    players: HashMap<IdType, UserData>,
    rooms: HashMap<IdType, RoomData>,     // The full list of the rooms.
    pub_rooms: HashSet<IdType>,           // Public rooms created for players that wants to play alone.
    pub_rooms_available: HashSet<IdType>, // Rooms that are not full.
    rng: ThreadRng,
}

impl Default for ServerActor {
    fn default() -> Self {
        ServerActor {
            players: HashMap::new(),
            rooms: HashMap::new(),
            pub_rooms: HashSet::new(),
            pub_rooms_available: HashSet::new(),
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

    fn create_room(&mut self, host_id: IdType, public: bool) -> IdType {
        let mut id;

        loop {
            id = self.rng.gen::<IdType>();

            if !self.rooms.contains_key(&id) {
                break;
            }
        }

        let mut players = HashSet::new();
        players.insert(host_id);
        let room = RoomData {
            state: RoomState::Matchmaking,
            players,
            in_game_count: 0,
            start_countdown_handle: None
        };
        self.rooms.insert(id, room);

        let host = self.players.get_mut(&host_id).unwrap();
        host.obj.is_host = true;
        host.room = Some(id);

        if public {
            self.pub_rooms.insert(id);
            self.pub_rooms_available.insert(id); // As soon as it is created, the pub room is available.
        }

        id
    }

    fn remove_room(&mut self, room_id: IdType) {
        self.rooms.remove(&room_id);
        self.pub_rooms.remove(&room_id);
        self.pub_rooms_available.remove(&room_id);

        //println!("room removed (id={}) because it's empty", room_id);
    }

    /// Send event to all users in the room
    fn broadcast_event(room: &RoomData, players_by_id: &HashMap<IdType, UserData>, event: OutEvent, skip_id: Option<IdType>) {
        ServerActor::broadcast_event_room(room, players_by_id, event, skip_id);
    }

    fn broadcast_event_room(room_data: &RoomData, players_by_id: &HashMap<IdType, UserData>, event: OutEvent, skip_id: Option<IdType>) {
        for id in room_data.players.iter() {
            if Some(*id) == skip_id {
                continue;
            }
            let player = match players_by_id.get(&id) {
                Some(x) => x,
                None => continue,
            };

            if player.in_game {
                continue; // Don't send if player is still in the game.
            }
            player.addr.do_send(Event(event.clone()));// TODO: remove clone
        }
    }

    fn leave_room_if_any(&mut self, ctx: &mut Context<Self>, player_id: IdType) {

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

        if room.players.len() < MIN_PLAYERS_PER_ROOM { // If the players count becomes lower than the min number of players stops the countdown.
            if room.cancel_start_countdown(ctx) {
                println!("[LeaveRoom] Room {}'s countdown has been canceled because a player quit.", room_id);
            }
        }

        // If the room is public and a player's quit and the number of players is less than the max, the room is available.
        if self.pub_rooms.contains(&room_id) && room.players.len() < MAX_PLAYERS_PER_ROOM {
            self.pub_rooms_available.insert(room_id);
        }

        if player.in_game {
            room.in_game_count -= 1;
        }

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

            // Why cant I convert a mutable reference to an immutable one? wtf
            // let room = &*room;
            //let room = self.rooms.get(&room_id).unwrap();

            let event = OutEvent::EventPlayerLeft {
                player: player_id.into(),
                new_host,
            };

            let in_game_event = OutGameEvent::PlayerLeft {
                player: player_id.into(),
                new_host
            };

            for id in room.players.iter() {
                let player = match self.players.get(&id) {
                    Some(x) => x,
                    None => continue,
                };

                if player.in_game {
                    player.addr.do_send(GameEvent(in_game_event.clone()));
                } else {
                    player.addr.do_send(Event(event.clone()));// TODO: remove clone
                }
            }
        } else {
            self.remove_room(room_id);
            println!("[LeaveRoom] Room {} has been deleted since all players quit.", room_id);
        }
    }

    fn find_available_room_for(&mut self, player_id: IdType, find_if: impl Fn(IdType, &RoomData) -> bool, max_iter: i32) -> Option<IdType> {
        let mut found = false;
        let mut found_room_id = 0;

        let mut iter = 0;
        for room_id in &self.pub_rooms_available {
            if max_iter > 0 && iter >= max_iter {
                break;
            }
            let room_data = self.rooms.get(&room_id).unwrap();
            if find_if(*room_id, room_data) {
                found = true;
                found_room_id = *room_id;
            }
            iter += 1;
        }

        if found {
            self.rooms.get_mut(&found_room_id).unwrap().players.insert(player_id);
            Some(found_room_id)
        } else {
            None
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
                    in_game: false,
                })
            }
        }

    }
}

impl Handler<Disconnect> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: Disconnect, ctx: &mut Context<Self>) -> Self::Result {
        self.leave_room_if_any(ctx, msg.id);
        self.players.remove(&msg.id);
    }
}

impl Handler<FindRoom> for ServerActor {
    type Result = FindRoomResult;

    fn handle(&mut self, msg: FindRoom, ctx: &mut Context<Self>) -> Self::Result {
        let my_id = msg.id;

        let mut just_created = false;

        let room_id = self.find_available_room_for(
            my_id, 
            |_, _| { true }, 
            -1
        );

        let room_id = match room_id {
            Some(room_id) => {
                ctx.notify(JoinRoom { id: my_id, room_id });
                room_id
            },
            None => {
                just_created = true;
                self.create_room(my_id, true)
            }
        };
        
        println!("[FindRoom] Room {} found for player {}.", room_id, my_id);

        FindRoomResult::Success {
            room_id,
            players: self.rooms.get(&room_id)
                .unwrap()
                .players
                .iter()
                .map(|x| self.players.get(x).expect("Cannot find player").obj.clone())
                .collect(),
            just_created
        }
    }
}

impl Handler<CreateRoom> for ServerActor {
    type Result = CreateRoomResult;

    fn handle(&mut self, msg: CreateRoom, ctx: &mut Context<Self>) -> Self::Result {
        self.leave_room_if_any(ctx, msg.id);
        let room_id = self.create_room(msg.id, false);
        let player = self.players.get_mut(&msg.id).expect("Cannot find player");
        CreateRoomResult {
            room_id,
            player: player.obj.clone()
        }
    }
}

impl Handler<JoinRoom> for ServerActor {
    type Result = JoinRoomResult;

    fn handle(&mut self, msg: JoinRoom, ctx: &mut Context<Self>) -> Self::Result {

        let my_id = msg.id;
        let room_id = msg.room_id;

        self.leave_room_if_any(ctx, my_id);

        let players_by_id = &mut self.players;

        let room_data = match self.rooms.get_mut(&room_id) {
            Some(room_data) => room_data,
            None => return JoinRoomResult::RoomNotFound
        };

        if room_data.state != RoomState::Matchmaking {
            return JoinRoomResult::AlreadyPlaying;
        }

        if room_data.players.len() >= MAX_PLAYERS_PER_ROOM {
            return JoinRoomResult::RoomIsFull;
        }

        room_data.players.insert(my_id);
        
        let user_data = players_by_id.get_mut(&my_id).unwrap();
        user_data.room = Some(room_id);

        let player = user_data.obj.clone();
        ServerActor::broadcast_event_room(
            &room_data, 
            players_by_id, 
            OutEvent::EventPlayerJoined { player }, 
            None
        );
        
        println!("[JoinRoom] Room {} joined by the player {}.", room_id, my_id);
        
        if room_data.players.len() == MIN_PLAYERS_PER_ROOM {
            let spawn_handle = ctx.notify_later(StartRoom {
                id: my_id,
                conn_type: RoomConnectionType::ServerBroadcast
            }, Duration::from_secs(ROOM_COUNTDOWN_ON_MIN_PLAYERS));
            room_data.start_countdown_handle = Some(spawn_handle);

            println!("[JoinRoom] Room {} has reached the min players ({}), it's going to start in {} seconds.", room_id, MIN_PLAYERS_PER_ROOM, ROOM_COUNTDOWN_ON_MIN_PLAYERS);
        }

        // If the max players are reached the room isn't available anymore (applies only if public).
        if room_data.players.len() == MAX_PLAYERS_PER_ROOM /*&& self.pub_rooms.contains(&room_id)*/ {
            self.pub_rooms_available.remove(&room_id);
        }

        JoinRoomResult::Success(
            room_data.players.iter().map(|id| players_by_id.get(id).unwrap().obj.clone()).collect()
        )
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

        ServerActor::broadcast_event(self.rooms.get(&room).unwrap(), &self.players, OutEvent::EventPlayerAvatarChange {
            player: id.into(),
            cosmetics: msg.obj,
        }, Some(msg.id));
    }
}

impl Handler<LeaveRoom> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: LeaveRoom, ctx: &mut Context<Self>) -> Self::Result {
        self.leave_room_if_any(ctx, msg.id);
    }
}

impl Handler<StartRoom> for ServerActor {
    type Result = ();

    fn handle(&mut self, msg: StartRoom, ctx: &mut Context<Self>) -> Self::Result {

        let room_id = match self.players.get(&msg.id).and_then(|x| x.room) {
            Some(x) => x,
            None => return,
        };

        println!("[StartRoom] Room {} is starting.", room_id);

        if let Some(room) = self.rooms.get_mut(&room_id) {

            // Ensures that there wasn't any "lobby" countdown running.
            room.cancel_start_countdown(ctx);

            // Removes the room from the pub rooms available since it has started (shouldn't be applied to private rooms).
            //if self.pub_rooms.contains(&room_id) {
                self.pub_rooms_available.remove(&room_id);
            //}

            if room.state != RoomState::Matchmaking || room.players.len() < 2 {
                return
            }

            room.state = RoomState::Playing;

            let event = OutEvent::EventRoomStart {
                connection_type: msg.conn_type,
                broadcast_id: format!("{}", room_id)
            };

            let room = if room.in_game_count > 0 {
                // Kick players that are still in-game
                let mut in_game_players = vec![];
                for id in room.players.iter() {
                    if let Some(x) = self.players.get_mut(&id) {
                        if x.in_game {
                            in_game_players.push(*id);
                        }
                    }
                }

                for id in in_game_players {
                    self.leave_room_if_any(ctx, id);
                }

                match self.rooms.get_mut(&room_id) {
                    None => return,
                    Some(x) => x,
                }
            } else {
                room
            };

            for id in room.players.iter() {
                if let Some(x) = self.players.get_mut(&id) {
                    x.in_game = true;
                    let _ = x.addr.do_send(Event(event.clone()));// TODO: remove clone
                }
            }
            room.in_game_count = room.players.len() as u32;
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
        let room = match player.room.and_then(|room| self.rooms.get(&room)) {
            Some(x) => x,
            None => return,
        };

        let raw = format!("{{\"sender\":\"{}\",{}", SerId(msg.sender_id), &msg.data[1..]);
        let raw_pkt = SendRelayMexRaw { data: raw };
        for player in room.players.iter() {
            if *player == msg.sender_id {
                continue;
            }
            let player = match self.players.get(&player) {
                Some(x) => x,
                None => continue,
            };
            if player.in_game {
                player.addr.do_send(raw_pkt.clone())
            }
        }
    }
}

impl Handler<GameEndRequest> for ServerActor {
    type Result = Option<GameEndAck>;

    fn handle(&mut self, msg: GameEndRequest, _ctx: &mut Context<Self>) -> Self::Result {
        let player = self.players.get_mut(&msg.id).expect("Invalid player");
        let rooms = &mut self.rooms;
        let room = match player.room.and_then(|x| rooms.get_mut(&x)) {
            Some(x) => x,
            None => return None,
        };

        if !player.in_game {
            return None;
        }

        room.state = RoomState::Matchmaking;
        player.in_game = false;
        room.in_game_count -= 1;

        let room = self.rooms.get(&player.room.unwrap()).unwrap();

        let users = room.players.iter()
            .map(|x| self.players.get(x).expect("Cannot find player").obj.clone())
            .collect();

        return Some(GameEndAck(users));
    }
}

