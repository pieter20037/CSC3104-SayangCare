use actix_web::{http::header, HttpResponse};

pub async fn index() -> HttpResponse {
    HttpResponse::Ok()
        .insert_header((header::CONTENT_TYPE, "text/html; charset=utf-8"))
        .body(include_str!("../../dashboard.html"))
}
