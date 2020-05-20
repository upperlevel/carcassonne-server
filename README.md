# carcassonne-server
The server code of [carcassonne](https://github.com/upperlevel/carcassonne).

### How to run
`BIND_ADDR="127.0.0.1:8081" cargo run --release`

It will take some time to compile but it's worth it.


### Protocols
You can find a description about the protocols in the protocols folder (we do not ensure you that they are updated though).
The server only manages the matchmaking, leaving a simpler relay protocol when the game starts.

### Performance
The server is quite fast but it has its own bottlenecks. I used the actor model in a quick and dirty way so now
every client has it's own actor and there's a single centralized actor that manages all of the lobbies.
To help spread out the load over multiple threads it may be helpful to create an actor for each room or maybe to go
the full non-actor route and create a concurrent hashmap (even if this seems quite hard to implement and probably a room
mutex will be inserted).

Actix does not support sending shared data so every time a message is broadcasted it has to be cloned (one time for each user).
This is quite heavy on resources and I'm trying to improve on that but there aren't many websocket libraries with that feature.
