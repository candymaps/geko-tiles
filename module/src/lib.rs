//! The tiles module (PROJECT.md §11), still a skeleton: it declares its routes, under `/tiles/`,
//! and answers 503 until it builds a part of the bundle.

use geko_module_sdk::Result;
use geko_module_sdk::module::{self, Context, Description, Method, Module, ModuleName, Request, Response, Route};

pub const NAME: &str = "tiles";

pub fn open(_: Context) -> Result<Box<dyn Module>> {
    Ok(Box::new(TilesModule))
}

pub struct TilesModule;

impl Module for TilesModule {
    fn describe(&self) -> Description {
        let route = |path: &str| Route { method: Method::Get, path: path.into(), pool: false, public: false };
        Description {
            name: ModuleName::new(NAME).expect("a valid name"),
            version: geko_module_sdk::module_version!().to_owned(),
            contract: module::CONTRACT,
            routes: vec![route("{z}/{x}/{y}"), route("tilejson.json")],
            openapi: None,
            attribution: None,
            commands: vec![],
        }
    }

    fn handle(&self, _: Request) -> Response {
        Response::error(503, "no tiles bundle loaded")
    }
}
