use actix_web::web;

use crate::client_ws::matchmaking_start;

pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api")
            .route("/matchmaking", web::get().to(matchmaking_start))
    );
}
