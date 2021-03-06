use std::fmt;
use std::mem;

use ::serde::{Deserialize, Serialize};
use serde::{Deserializer, Serializer};
use serde::de;
use serde::de::Visitor;
use serde::export::Formatter;
use std::borrow::Cow;

pub type IdType = usize;

// Common data

#[derive(Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlayerObject {
    pub id: SerId,
    pub username: String,
    #[serde(flatten)]
    pub cosmetics: PlayerCosmetics,
    pub is_host: bool,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct LoginData {
    pub username: String,
    #[serde(flatten)]
    pub cosmetics: PlayerCosmetics,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct PlayerCosmetics {
    pub avatar: u32,
    pub color: u64,
}

// Client to Server data

#[derive(Deserialize)]
pub struct IdMessage {
    pub id: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReceivedMessage {
    Login {
        details: LoginData
    },
    ChangeAvatar {
        #[serde(flatten)]
        cosmetics: PlayerCosmetics,
    },
    RoomFind {  
    },
    RoomCreate {
    },
    RoomLeave {
    },
    #[serde(rename_all = "camelCase")]
    RoomJoin {
        invite_id: SerId,
    },
    #[serde(rename_all = "camelCase")]
    RoomStart {
        connection_type: RoomConnectionType,
    },
    #[serde(rename_all = "camelCase")]
    EventRoomStartAck {
        request_id: u64,
    }
}


#[derive(Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoomConnectionType {
    ServerBroadcast,
}


// Server to Client Data
#[derive(Serialize)]
pub struct OutMessage<T: Serialize> {
    pub id: u64,
    #[serde(flatten)]
    pub mex: T,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response<'a, 'b, T: Serialize> {
    #[serde(rename = "type")]
    pub ptype: Cow<'a, str>,
    pub request_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Cow<'b, str>>,
    #[serde(flatten)]
    pub data: T,
}

impl<'a, 'b, T: Serialize> Response<'a, 'b, T> {
    pub fn ok(req_id: u64, ptype: Cow<'a, str>, data: T) -> Self {
        Response {
            ptype,
            request_id: req_id,
            result: Some(Cow::Borrowed("ok")),
            data
        }
    }

    pub fn from(request_id: u64, ptype: Cow<'a, str>, result: Option<Cow<'b, str>>, data: T) -> Self {
        Response {
            ptype, request_id, result, data
        }
    }
}

#[derive(Serialize)]
pub struct NoData {}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutEvent {
    EventPlayerJoined {
        player: PlayerObject,
    },
    #[serde(rename_all = "camelCase")]
    EventPlayerLeft {
        player: SerId,
        #[serde(skip_serializing_if = "Option::is_none")]
        new_host: Option<SerId>,
    },
    EventPlayerAvatarChange {
        player: SerId,
        #[serde(flatten)]
        cosmetics: PlayerCosmetics
    },
    #[serde(rename_all = "camelCase")]
    EventRoomStart {
        connection_type: RoomConnectionType,
        broadcast_id: String,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub player_id: SerId,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomFindResponse {
    pub players: Vec<PlayerObject>,
    pub room_id: SerId,
    pub just_created: bool
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomCreateResponse {
    pub players: [PlayerObject; 1],
    pub invite_id: SerId,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomJoinResponse {
    pub players: Vec<PlayerObject>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Error<'a, 'b> {
    #[serde(rename = "type")]
    pub mtype: &'static str,// always "error"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_id: Option<u64>,
    pub error: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<Cow<'b, str>>,
}

impl <'a, 'b> Error<'a, 'b> {
    pub fn from_origin(origin_id: u64, error: Cow<'a, str>, error_message: Option<Cow<'b, str>>) -> Self {
        Error {
            mtype: "error",
            origin_id: Some(origin_id),
            error,
            error_message
        }
    }

    pub fn from(error: Cow<'a, str>, error_message: Option<Cow<'b, str>>) -> Self {
        Error {
            mtype: "error",
            origin_id: None,
            error,
            error_message
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct SerId(pub IdType);

impl fmt::Display for SerId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let data: [u8; mem::size_of::<IdType>()] = self.0.to_be_bytes();
        let str = base64::encode(data);
        f.write_str(&str)
    }
}

impl Serialize for SerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: Serializer,
    {
        let data: [u8; mem::size_of::<IdType>()] = self.0.to_be_bytes();
        let str = base64::encode(data);
        serializer.serialize_str(&str)
    }
}

struct SerIdVisitor;

impl<'de> Visitor<'de> for SerIdVisitor {
    type Value = SerId;

    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        formatter.write_str("an ID")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error {
        let data = match base64::decode(v) {
            Ok(x) => x,
            Err(_) => return Err(E::custom("Invalid ID"))
        };

        if data.len() != mem::size_of::<IdType>() {
            return Err(E::custom("Invalid ID length"));
        }
        let mut u64_data = [0; mem::size_of::<IdType>()];
        u64_data[..].copy_from_slice(&data);
        Ok(SerId(IdType::from_be_bytes(u64_data)))
    }
}

impl<'de> Deserialize<'de> for SerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de> {
       deserializer.deserialize_str(SerIdVisitor)
    }
}

impl From<IdType> for SerId {
    fn from(x: IdType) -> Self {
        SerId(x)
    }
}

impl From<SerId> for IdType {
    fn from(x: SerId) -> Self {
        x.0
    }
}


/// --------------------- IN-GAME PROTOCOL ---------------------

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReceivedGameMessage {
    EndGame {},
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutGameMessage {
    EndGameAck {
        players: Vec<PlayerObject>
    },
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutGameEvent {
    #[serde(rename_all = "camelCase")]
    PlayerLeft {
        player: SerId,
        #[serde(skip_serializing_if = "Option::is_none")]
        new_host: Option<SerId>,
    },
}
