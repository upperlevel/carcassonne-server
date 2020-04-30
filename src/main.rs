#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;


use actix::prelude::*;
use actix_web::{HttpServer, App, Responder, HttpResponse, web};
use env_logger;


mod api_service;
mod client_ws;
mod protocol;
mod server_actor;
mod room_db_async;

async fn index() -> impl Responder {
    HttpResponse::Ok().body("Hello world!")
}


#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let room_db = server_actor::ServerActor::default().start();

    HttpServer::new(move || {
        App::new()
            .data(room_db.clone())
            .configure(api_service::config)
            .route("/", web::get().to(index))
    })
        .bind("0.0.0.0:8080")?
        .run()
        .await
}
