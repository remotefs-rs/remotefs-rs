use std::net::TcpListener;

use actix_web::dev::Server;
use actix_web::web::Data;
use actix_web::{App as ActixApp, HttpServer};

mod health_check;

pub struct WebServer {
    server: Server,
}

struct WebserverData {}

impl WebServer {
    /// Initialize web server
    pub async fn init(web_port: u16) -> anyhow::Result<Self> {
        info!("webserver initialized");
        info!("web port: {web_port}");

        let listener = TcpListener::bind(format!("0.0.0.0:{web_port}"))?;

        let server = {
            HttpServer::new(move || {
                let web_data = Data::new(WebserverData {});
                ActixApp::new()
                    .service(health_check::check_action)
                    .app_data(web_data)
            })
            .listen(listener)?
            .run()
        };

        info!("web server initialized");
        Ok(Self { server })
    }

    /// Run web server
    pub async fn run(self) -> anyhow::Result<()> {
        info!("running web server");
        self.server.await?;
        info!("web server stopped");
        Ok(())
    }
}
