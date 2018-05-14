use atomic_immut::AtomicImmut;
use bytecodec::json_codec::{JsonDecoder, JsonEncoder};
use bytecodec::null::{NullDecoder, NullEncoder};
use config::Config;
use fibers::{Executor, InPlaceExecutor, Spawn};
use fibers_http_server::metrics::MetricsHandler;
use fibers_http_server::{HandleRequest, Reply, Req, Res, ServerBuilder, Status};
use futures::future::ok;
use futures::Future;
use httpcodec::{BodyDecoder, BodyEncoder};
use prometrics;
use slog::Logger;
use std;
use std::net::SocketAddr;
use std::sync::Arc;

pub fn start_server(
    logger: Logger,
    port: u16,
    config: Arc<AtomicImmut<Config>>,
) -> Result<(), Box<std::error::Error>> {
    let executor = InPlaceExecutor::new()?;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let mut builder = ServerBuilder::new(addr);
    builder.add_handler(GetConfigHandler(Arc::clone(&config)))?;
    builder.add_handler(PutConfigHandler { logger, config })?;

    // Enables process metrics and registers a HTTP endpoint for exporting metrics
    prometrics::default_registry().register(prometrics::metrics::ProcessMetricsCollector::new());
    builder.add_handler(MetricsHandler)?;

    // Starts HTTP server
    let http_server = builder.finish(executor.handle());
    executor.spawn(http_server.map_err(|e| panic!("{}", e)));
    std::thread::spawn(move || {
        if let Err(e) = executor.run() {
            panic!("{}", e);
        }
    });
    Ok(())
}

struct GetConfigHandler(Arc<AtomicImmut<Config>>);
impl HandleRequest for GetConfigHandler {
    const METHOD: &'static str = "GET";
    const PATH: &'static str = "/config";

    type ReqBody = ();
    type ResBody = Config;
    type Decoder = BodyDecoder<NullDecoder>;
    type Encoder = BodyEncoder<JsonEncoder<Config>>;
    type Reply = Reply<Self::ResBody>;

    fn handle_request(&self, _req: Req<Self::ReqBody>) -> Self::Reply {
        let config = (*self.0.load()).clone();
        Box::new(ok(Res::new(Status::Ok, config)))
    }
}

struct PutConfigHandler {
    logger: Logger,
    config: Arc<AtomicImmut<Config>>,
}
impl HandleRequest for PutConfigHandler {
    const METHOD: &'static str = "PUT";
    const PATH: &'static str = "/config";

    type ReqBody = Config;
    type ResBody = ();
    type Decoder = BodyDecoder<JsonDecoder<Config>>;
    type Encoder = BodyEncoder<NullEncoder>;
    type Reply = Reply<Self::ResBody>;

    fn handle_request(&self, req: Req<Self::ReqBody>) -> Self::Reply {
        let config = req.into_body();
        self.config.store(config.clone());
        info!(self.logger, "new config: {:?}", config);

        Box::new(ok(Res::new(Status::Ok, ())))
    }
}
