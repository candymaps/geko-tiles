//! The tiles module (docs/MODULE_PLAN.md): it downloads the OSM extracts of the scope and the
//! global inputs of the Protomaps profile, runs Planetiler once and converts its tiles into its part
//! of the bundle, with the fonts and sprites of the styles. It serves them under `/tiles/`, with the
//! styles of Protomaps (`serve`, `styles`).

mod assets;
mod config;
mod extracts;
mod geo;
#[doc(hidden)]
pub mod golden;
mod inputs;
mod pbf;
mod pipeline;
mod planetiler;
mod pmtiles;
mod serve;
mod stats;
mod shapefile;
mod styles;
mod tileset;

use std::time::Duration;

use geko_module_sdk::Result;
use geko_module_sdk::module::{self, Built, BundleView, Context, Description, Method, Module, ModuleName, Purge, Request, Response, Route};

use crate::pipeline::Setup;

pub const NAME: &str = "tiles";

pub fn open(context: Context) -> Result<Box<dyn Module>> {
    let config = config::Config::parse(&context.config, &context.config_file)?;
    let server = serve::Server::new(&config.url).with_settings(settings(&config)).with_data_dir(context.data_dir.clone());
    Ok(Box::new(TilesModule { setup: Setup { config, scope: context.scope, data_dir: context.data_dir, data: context.data }, server }))
}

/// Every option of `geko-tiles.toml` with the value it has and the one it would have without a file,
/// and when a change of it takes effect.
fn settings(config: &config::Config) -> serde_json::Value {
    let d = config::Config::default();
    let setting = |key: &str, value: serde_json::Value, default: serde_json::Value, apply: &str| {
        serde_json::json!({ "key": key, "value": value, "default": default, "apply": apply })
    };
    serde_json::Value::Array(vec![
        setting("format", config.format.as_str().into(), d.format.as_str().into(), "rebuild"),
        setting("url", config.url.clone().into(), d.url.into(), "restart"),
        setting("osm.mirror", config.osm.mirror.clone().into(), d.osm.mirror.into(), "rebuild"),
        setting("planetiler.memory", config.planetiler.memory.clone().into(), d.planetiler.memory.into(), "rebuild"),
        setting("planetiler.threads", config.planetiler.threads.into(), d.planetiler.threads.into(), "rebuild"),
        setting("planetiler.storage", config.planetiler.storage.as_str().into(), d.planetiler.storage.as_str().into(), "rebuild"),
        setting("planetiler.qrank", config.planetiler.qrank.into(), d.planetiler.qrank.into(), "rebuild"),
    ])
}

pub struct TilesModule {
    setup: Setup,
    server: serve::Server,
}

impl Module for TilesModule {
    fn describe(&self) -> Description {
        let route = |path: &str| Route { method: Method::Get, path: path.into(), pool: false, public: false };
        Description {
            name: ModuleName::new(NAME).expect("a valid name"),
            version: geko_module_sdk::module_version!().to_owned(),
            contract: module::CONTRACT,
            routes: vec![
                route("{z}/{x}/{y}"),
                route("tilejson.json"),
                route("styles.json"),
                route("styles/{flavor}/{lang}"),
                route("fonts/{fontstack}/{range}"),
                route("sprites/{name}"),
                route("status.json"),
                route("stats.json"),
            ],
            openapi: None,
            attribution: Some(serde_json::json!({
                "produced_work": {
                    "license": "ODbL 1.0",
                    "notice": "The tiles are a Produced Work of OpenStreetMap: © OpenStreetMap contributors.",
                },
            })),
            commands: vec![],
        }
    }

    fn plan(&self) -> Result<String> {
        Ok(pipeline::plan(&self.setup, &pipeline::downloader(&self.setup, false)))
    }

    fn fetch(&self, refresh: bool) -> Result<String> {
        pipeline::fetch(&self.setup, &pipeline::downloader(&self.setup, refresh))
    }

    fn ready(&self) -> Result<()> {
        pipeline::licenses_accepted(&self.setup)
    }

    fn build(&self, force: bool, refresh: bool) -> Result<Built> {
        pipeline::build(&self.setup, &pipeline::downloader(&self.setup, refresh), force)
    }

    fn published(&self) -> Result<String> {
        pipeline::published(&self.setup)
    }

    fn next_build(&self) -> Option<Duration> {
        pipeline::next_build(&self.setup)
    }

    fn purge(&self, what: Purge) -> Result<String> {
        match what {
            Purge::Data => pipeline::purge(&self.setup),
            Purge::Packages { .. } => Ok(String::new()),
        }
    }

    fn load(&self, bundle: &BundleView) -> Result<()> {
        self.server.load(bundle)
    }

    fn handle(&self, request: Request) -> Response {
        self.server.handle(&request)
    }

    fn metrics(&self, out: &mut String) {
        self.server.metrics(out);
    }

    fn shutdown(&self) -> Result<()> {
        self.server.shutdown();
        Ok(())
    }
}
