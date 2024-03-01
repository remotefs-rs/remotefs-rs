use actix_web::{get, HttpResponse};

#[get("/check")]
pub async fn check_action() -> HttpResponse {
    HttpResponse::Ok().content_type("application/json").finish()
}

#[cfg(test)]
mod test {
    use actix_web::dev::Service;
    use actix_web::{test, App};

    use super::*;

    #[actix_rt::test]
    async fn test_check_action() {
        crate::test::log_init();
        let app = test::init_service(App::new().service(check_action)).await;
        let request = test::TestRequest::with_uri("/check").to_request();

        let res = app.call(request).await.unwrap();
        assert!(res.status().is_success());
    }
}
