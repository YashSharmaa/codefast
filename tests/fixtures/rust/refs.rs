//! References: calls, receivers, member access, constructors, patterns and locals.
use crate::net::{self, Config, Handler};
use std::sync::{Arc, Mutex};

static REGISTRY: Mutex<Registry> = Mutex::new(Registry::new());
const RETRIES: u32 = 3;

pub struct Server {
    cfg: Arc<Config>,
    handler: fn(&Request) -> Response,
    peers: Vec<Peer>,
    state: RefCell<State>,
    last: Option<Request>,
}

impl Server {
    pub fn new(cfg: Config, peers: &[Peer]) -> Self {
        let cfg = Arc::new(cfg);
        Self { cfg, handler: default_handler, peers: peers.to_vec(), state: RefCell::new(State::default()), last: None }
    }

    fn run(&mut self, req: &Request, out: &mut dyn Sink, limit: Option<Limit>) -> Result<u32, net::Error> {
        self.cfg.load();
        self.check(req)?;
        let rsp = (self.handler)(req);
        out.write(&rsp);
        let n = Self::count(&self.peers, RETRIES);
        self.last = Some(req.clone());
        self.cfg.timeout = n as Duration;
        self.handler = net::fallback;
        self.state.borrow_mut().advance(n);
        if let Some(max) = limit {
            max.enforce(n);
        }
        let first = self.peers.first().unwrap();
        first.ping();
        for peer in &self.peers {
            peer.ping();
        }
        for (i, p) in self.peers.iter().enumerate() {
            p.send(i);
        }
        let guard = REGISTRY.lock().unwrap();
        guard.register(self);
        helper(n, crate::util::clamp(n, 0, 9));
        net::transport::open::<Tcp>(self.cfg.port).await?;
        <Self as Handler>::handle(self, req);
        Ok(n)
    }

    fn check(&self, req: &Request) -> Result<(), net::Error> {
        let Request { path, method: m, .. } = req;
        path.normalize();
        match m {
            Method::Get | Method::Head => {}
            Method::Post { body, .. } if body.len() > MAX_BODY => return Err(net::Error::TooLarge(body.len())),
            other @ Method::Custom(_) => other.audit(),
            UNKNOWN => {}
            _ => unreachable!(),
        }
        while let Some(next) = self.last.take() {
            next.replay();
        }
        Ok(())
    }

    fn count(peers: &[Peer], floor: u32) -> u32 {
        let mut total = floor;
        let bump = |p: &Peer, extra| p.weight() + extra;
        peers.iter().for_each(|p| total += bump(p, 1));
        total
    }
}

impl Handler for Server {
    fn handle(&self, req: &Request) -> Response {
        let mut rsp = Response::default();
        rsp.status = Status::Ok;
        rsp.headers.insert("x", req.path.to_string());
        let body: Box<Body> = Box::new(Body::from(req));
        body.seal();
        let copy = rsp.clone();
        copy.send();
        let boxed = Box::new(Wrapper { inner: copy, tag: 1 });
        boxed.inner.flush();
        let text = String::from("x");
        text.len();
        "lit".trim();
        rsp
    }
}

fn generic<T: Visitor, U>(v: &mut T, items: Vec<Item>, maybe: Option<Item>, cb: impl Fn(u32) -> U) -> Vec<U>
where
    U: Into<Wide> + Send,
{
    v.visit(&items[0]);
    items[1].touch();
    maybe.unwrap().touch();
    maybe.is_some();
    let x = cb(1);
    x.into();
    let n = items.len() as u64;
    let made = make_item();
    made.touch();
    let v2 = items.iter().map(Item::id).collect::<Vec<Id>>();
    T::default_visitor();
    U::from(n);
    COUNTER.fetch_add(1, Ordering::SeqCst);
    unsafe { GLOBAL = n };
    Vec::new()
}

fn shadow(count: u32) {
    let count = count + 1;
    {
        let helper = |x: u32| x;
        helper(count);
    }
    helper(count, 2);
    let (a, [b, c], Point { x, y: py }) = split();
    use_all(a, b, c, x, py);
    let cfg = Config { retries: RETRIES, on_error: log::error, ..Config::base() };
    cfg.retries;
}
