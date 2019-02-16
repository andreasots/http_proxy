use futures::future::Either;
use futures::{Future, IntoFuture, Stream};
use hyper::client::HttpConnector;
use hyper::service::service_fn;
use hyper::{Body, Client, HeaderMap, Method, Request, Response, Server, StatusCode, Uri};
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Cache {
    cache: Arc<Mutex<HashMap<(Method, Uri), (StatusCode, HeaderMap, Vec<u8>)>>>,
    client: Client<HttpConnector>,
}

impl Cache {
    fn new() -> Cache {
        Cache {
            cache: Arc::new(Mutex::new(HashMap::new())),
            client: Client::new(),
        }
    }

    fn handle(
        self,
        req: Request<Body>,
    ) -> Box<dyn Future<Item = Response<Body>, Error = Box<dyn Error + Send + Sync + 'static>> + Send>
    {
        println!("{} {}", req.method(), req.uri());

        match *req.method() {
            Method::CONNECT => Box::new(self.handle_connect(req).from_err()),
            Method::GET | Method::HEAD => Box::new(self.handle_cacheable(req).from_err()),
            _ => Box::new(self.handle_uncacheable(req).from_err()),
        }
    }

    fn handle_connect(
        self,
        _req: Request<Body>,
    ) -> impl Future<Item = Response<Body>, Error = Box<dyn Error + Send + Sync + 'static>> {
        let mut res = Response::new(Body::from("CONNECT is not supported"));
        *res.status_mut() = StatusCode::BAD_REQUEST;
        Ok::<_, Box<dyn Error + Send + Sync + 'static>>(res).into_future()
    }

    fn handle_cacheable(
        self,
        req: Request<Body>,
    ) -> impl Future<Item = Response<Body>, Error = impl Error + Send + Sync + 'static> {
        let cached = self
            .cache
            .lock()
            .expect("cache mutex poisoned")
            .get(&(req.method().clone(), req.uri().clone()))
            .cloned();
        if let Some((status_code, headers, body)) = cached {
            println!("Cache hit for {} {}", req.method(), req.uri());
            let mut res = Response::new(Body::from(body));
            *res.status_mut() = status_code;
            res.headers_mut().extend(headers);
            Either::A(Ok(res).into_future())
        } else {
            println!("Cache miss for {} {}", req.method(), req.uri());
            let cache_key = (req.method().clone(), req.uri().clone());
            let future = self
                .client
                .request(req)
                .and_then(move |res| {
                    let status = res.status().clone();
                    let headers = res.headers().clone();

                    res.into_body()
                        .concat2()
                        .map(move |body| (status, headers, body.into_bytes().to_vec()))
                })
                .map(move |(status, headers, body)| {
                    self.cache
                        .lock()
                        .expect("cache mutex poisoned")
                        .insert(cache_key, (status, headers.clone(), body.clone()));

                    let mut res = Response::new(Body::from(body));
                    *res.status_mut() = status;
                    res.headers_mut().extend(headers);
                    res
                });
            Either::B(future)
        }
    }

    fn handle_uncacheable(
        self,
        req: Request<Body>,
    ) -> impl Future<Item = Response<Body>, Error = impl Error + Send + Sync + 'static> {
        self.client.request(req)
    }
}

fn main() {
    let addr = ([127, 0, 0, 1], 3000).into();

    let cache = Cache::new();

    let make_service = move || {
        let cache = cache.clone();
        service_fn(move |req| cache.clone().handle(req))
    };

    let server = Server::bind(&addr).serve(make_service);

    hyper::rt::run(server.map_err(|e| {
        eprintln!("server error: {}", e);
    }));
}
