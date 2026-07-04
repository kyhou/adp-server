mod game;
mod purchase;
mod upload;
mod well_known;

use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/.well-known/adp", get(well_known::get_well_known))
        .route("/upload", post(upload::upload_handler))
        .route("/purchase/confirm", post(purchase::confirm_handler))
        .route("/game/{game_coordinate}", get(game::download_handler))
        .with_state(state)
}
