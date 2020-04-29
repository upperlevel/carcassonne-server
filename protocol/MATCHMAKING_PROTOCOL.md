

## General
Every message has an Id that identifies it so that the responses can be sent out of order
The response will have an id of the original request.

### Misc Data
```
PlayerObject {
    id: String,
    username: String,
    color: Int,
    border_color: Int,
    host: bool
}
```


### Login
Once the connection has begun the only action that the client can do is to login,
after the login has been successful the client can no longer log in but he can begin the matchmaking
part. 

The "details" must not contain neither "id" nor "host" field as the server will 

Client -> Server
```json
{
  "id": id,
  "type": "login",
  "details": PlayerObject
}
```

Response:
Client <- Server
```json
{
  "id": id,
  "type": "login_response",
  "requestId": <original request id>,
  "result": "ok",
  "playerId": <player id>
}
```


### Init room
Client -> Server
```json
{
  "id": id,
  "type": "room_create"
}
```

Res:
Client <- Server
```json
{
  "id": id,
  "type": "room_create_response",
  "requestId": <original request id>
  "result":  "ok",
  "players": Array<PlayerObject>,// array of 1 element
  "inviteId": invite_id
}
```
Possible errors:
- Name already taken
- Invalid name


### Leave room
Client -> Server

```json
{
  "id": id,
  "type": "room_leave",
  "new_host": <player_id>
}
```

The new_host field is optional and will only be inserted if the old host left the group.
If it's present it'll contain the id of the new host.

Response:
Client <- Server
```json
{ 
  "id": id,
  "type": "room_leave_response",
  "request_id": <original request id>,
  "result": "ok"
}
```

### Join room
Client -> Server

```json
{
  "id": id,
  "type": "room_join",
  "inviteId": invite_id
}
```

Response:
Client <- Server
```json
{ 
  "id": id,
  "type": "room_join_response",
  "requestId": <original request id>,
  "result": "ok",
  "players": Array<PlayerObject>
}
```

Possible Errors (written in the "result" field):
- `room_not_found`: The requestId is not valid (the room could've been closed).
- `name_conflict`: Another player has your same name.
- `already_playing`: You canot join a room if the game is started already.

### Start room
Client -> Server

Host only
```json
{
  "id": id,
  "type": "room_start",
  "connectionType": "server_broadcast"
}
```

## Events
### Room player join
Server -> Client
```json
{
  "id": id,
  "type": "event_player_joined",
  "player": <PlayerObject>
}
```

### Room player leave
Server -> Client
```json
{
  "id": id,
  "type": "event_player_left",
  "player": <PlayerId>
}
```

### Starting room
Server -> Client
```json
{
  "id": id,
  "type": "event_room_start"
}
```

Server <- Client
```json
{
  "id": id,
  "type": "event_room_start_acknowledge",
  "responseId": <original request id>
}
```

After the "event_room_start_acknowledge" packet is received the connection will be used as explained in the
RELAY_PROTOCOL.md file. The client must pay attention if he is sending packets asynchronously as if a packet
is sent after the ack it will be broadcasted to every player without any server processing. 
