//! The consumer end of the hub.
//!
//! The shim dials `ConsumerApi` — the audience-split service a reader/router
//! peer is meant to see
//! (`hub:ba47c87abc0f961967d5ccfa9254945075c16c1e9a3b8eaed1e8195e1663c241`). It
//! never dials `NodeIngress`, and it never reaches a node directly: a node is
//! addressed only by registry name through the hub
//! (`hub:bf406db5b660bd595cccdfcec5921121bb5a1ac9281a0d9a5c28be2d7ccc79b4`).
//!
//! implements: 121ac6ebe48b717b93e775f5a0526076a9230ec0e10e748dbcbaf181bf758120
//! implements: 1d90deaa708a39eb606d0190ebbbab5169902bb5fb78a8e76acff87904e91414
//! implements: deda3d32832baaaaeb5814a0f1a859f80451d081d5f8e70d1def0a3d989c9dfe

use std::sync::Arc;
use std::time::Duration;

use hub_protocol::discovery::TopologySnapshot;
use hub_protocol::errors::RouteError;
use hub_protocol::routing::{RoutedCall, RoutedReply};
use hub_protocol::service::ConsumerApiClient;
use tokio::sync::RwLock;

use crate::error::ShimError;
use crate::route::{RouteFailure, Target};

/// How long a call waits before the shim gives up **waiting**.
///
/// Generous, because the two ways of being wrong are not symmetric: too long
/// merely delays, while too short reports a timeout on work that was about to
/// succeed — and a timeout is the answer a caller is most likely to mishandle.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

struct ClientLane {
    connection: vox::ConnectionHandle,
    client: ConsumerApiClient,
}

/// A live consumer connection to one hub.
///
/// Holds no mesh state of its own: every read goes to the hub, so two readers
/// never disagree about the world. The lane itself is replaceable — a hub that
/// restarts is re-dialed rather than ending the session.
// implements: b37a9e70ffbc60eea24a22bf9bb6d90244e1368ef7e4f2006893f71cc6317b07
pub struct HubClient {
    addr: String,
    /// Behind a lock so a dead lane can be swapped for a fresh one. The read
    /// lock is held only long enough to clone the handle, never across a call,
    /// so concurrent calls do not serialize behind each other.
    client: RwLock<Arc<ClientLane>>,
    timeout: Duration,
}

impl Drop for HubClient {
    fn drop(&mut self) {
        let _ = self.client.get_mut().connection.shutdown();
    }
}

impl std::fmt::Debug for HubClient {
    /// Only the address. The underlying lane is a live connection, not
    /// information, and printing it would put transport internals in messages
    /// that are meant to be read by a person or an agent.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubClient")
            .field("addr", &self.addr)
            .finish()
    }
}

/// Open one consumer lane to `addr`, retaining its connection driver.
async fn dial_lane(addr: &str) -> Result<ClientLane, ShimError> {
    let socket = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|source| ShimError::HubUnreachable {
            addr: addr.to_string(),
            source,
        })?;

    let connection = vox::initiator_on(vox::transport::tcp::StreamLink::tcp(socket))
        .establish_connection()
        .await
        .map_err(|e| ShimError::HubHandshake {
            addr: addr.to_string(),
            detail: e.to_string(),
        })?;
    let client = connection
        .open_lane()
        .await
        .map_err(|e| ShimError::HubHandshake {
            addr: addr.to_string(),
            detail: e.to_string(),
        })?;
    Ok(ClientLane { connection, client })
}

/// Was this the connection dying, rather than someone answering?
///
/// Only a transport failure is worth re-dialing for. `VoxError::User` carries a
/// **domain** answer — a refusal, an unknown node — which reached someone and
/// will say the same thing again.
fn is_transport_failure<E>(e: &vox::VoxError<E>) -> bool {
    !matches!(e, vox::VoxError::User(_))
}

impl HubClient {
    /// Dial the hub at `addr` and open a consumer lane.
    ///
    /// A failure here is terminal for the session rather than something to
    /// degrade around — there is no node-local path to fall back to.
    pub async fn dial(addr: &str) -> Result<Self, ShimError> {
        Ok(Self {
            addr: addr.to_string(),
            client: RwLock::new(Arc::new(dial_lane(addr).await?)),
            timeout: DEFAULT_CALL_TIMEOUT,
        })
    }

    /// Bound every call by `timeout` instead of the default.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The hub address this client is talking to.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// How long a call waits before giving up waiting.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The lane as it stands now.
    async fn lane(&self) -> Arc<ClientLane> {
        self.client.read().await.clone()
    }

    /// Replace the lane — unless someone else already did.
    ///
    /// Two calls failing at once must not produce two re-dials: the second finds
    /// the stored lane is no longer the one it was holding, and takes that.
    async fn redial(&self, stale: &Arc<ClientLane>) -> Result<Arc<ClientLane>, ShimError> {
        let mut guard = self.client.write().await;
        if !Arc::ptr_eq(&guard, stale) {
            return Ok(guard.clone());
        }
        let fresh = Arc::new(dial_lane(&self.addr).await?);
        let old = std::mem::replace(&mut *guard, fresh.clone());
        let _ = old.connection.shutdown();
        Ok(fresh)
    }

    /// Bound a call by this client's timeout.
    ///
    /// This stops the **waiting**, not the work — which is why `ShimError`'s
    /// timeout variant says so in as many words.
    async fn bounded<T>(
        &self,
        what: &str,
        future: impl std::future::Future<Output = T>,
    ) -> Result<T, ShimError> {
        tokio::time::timeout(self.timeout, future)
            .await
            .map_err(|_| ShimError::TimedOut {
                what: what.to_string(),
                after: self.timeout,
            })
    }

    /// A point-in-time view of the connected mesh.
    ///
    /// Reads are link-agnostic, so no link selection is involved
    /// (`hub:08c3ce357742ccf578caddcfb6578a92216b515adcfe1ac9de4543406be52cb0`).
    pub async fn topology(&self) -> Result<TopologySnapshot, ShimError> {
        let lane = self.lane().await;
        match self.bounded("topology", lane.client.topology()).await? {
            Ok(snapshot) => Ok(snapshot),
            Err(e) if is_transport_failure(&e) => {
                // The lane died. Re-dial once and ask again; if the dial fails,
                // that error names the address, which is what to act on.
                let fresh = self.redial(&lane).await?;
                self.bounded("topology", fresh.client.topology())
                    .await?
                    .map_err(|e| ShimError::Hub(format!("{e:?}")))
            }
            Err(e) => Err(ShimError::Hub(format!("{e:?}"))),
        }
    }

    /// Route one call to a connector on the selected node/link.
    ///
    /// The payload is **opaque here** — its meaning is a private agreement
    /// between the connector and its callers, and neither the hub nor this shim
    /// decodes it
    /// (`hub:d78fb95fddc31aa42bc927da1eaed44292529b3671d1ef9ef58ec8cd0858e51c`).
    /// That is also what keeps a component-version mismatch harmless: bytes
    /// cross the boundary, never `Facet`-derived types.
    ///
    /// A failure keeps **which side failed**, because that is the only part a
    /// caller can act on
    /// (`hub:45af8b01ab1929e94682ed82013a4aad0b48af81c61b3dea89d7b8323cf94752`).
    /// A **domain** failure is returned as-is and never retried: it reached
    /// someone and was answered, and retrying a `start` would run the work
    /// twice.
    pub async fn route(&self, target: Target, call: RoutedCall) -> Result<RoutedReply, ShimError> {
        let what = format!(
            "{}.{} on node '{}'",
            call.connector, call.method, target.node
        );
        let lane = self.lane().await;

        let error = match self
            .bounded(&what, lane.client.route(target.selector(), call.clone()))
            .await?
        {
            Ok(reply) => return Ok(reply),
            Err(e) if is_transport_failure(&e) => {
                let fresh = self.redial(&lane).await?;
                match self
                    .bounded(&what, fresh.client.route(target.selector(), call.clone()))
                    .await?
                {
                    Ok(reply) => return Ok(reply),
                    Err(e) => e,
                }
            }
            Err(e) => e,
        };

        let (side, detail) = match &error {
            vox::VoxError::User(route_error) => match route_error.as_ref() {
                RouteError::Hub(hub) => (RouteFailure::Hub, format!("{hub:?}")),
                RouteError::Node(node) => (RouteFailure::Node, format!("{node:?}")),
            },
            // Transport-level, and a re-dial did not help. From the caller's
            // point of view the hub could not deliver.
            other => (RouteFailure::Hub, format!("{other:?}")),
        };
        Err(ShimError::Route {
            side,
            node: target.node,
            connector: call.connector,
            method: call.method,
            detail,
        })
    }
}
