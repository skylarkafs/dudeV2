pub mod icmp;
pub mod network;

use crate::error::ServerError;
use crate::snmp::{ResolvedCredential, SnmpClient};
use nextdude_common::proto::v1;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub device_id: i64,
    pub host: String,
    pub resolved_ip: Option<IpAddr>,
    pub snmp_port: u16,
    pub snmp_timeout: Duration,
    pub snmp_retries: u32,
    pub snmp_credential: ResolvedCredential,
}

#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub success: bool,
    pub rtt_ms: f64,
    pub value: f64,
    pub text: String,
    pub force_partial: bool,
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Abstraction for executing probes locally or via future remote probe agents.
pub trait ProbeExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        target: &'a ProbeTarget,
        probe: &'a v1::ProbeDefinition,
        interface_key: &'a str,
    ) -> BoxFuture<'a, Result<ProbeOutcome, ServerError>>;
}

#[derive(Clone)]
pub struct LocalProbeExecutor {
    pub icmp: Arc<icmp::IcmpProber>,
    pub snmp: SnmpClient,
    pub http: reqwest::Client,
    pub dns: Option<hickory_resolver::TokioAsyncResolver>,
}

impl LocalProbeExecutor {
    pub fn new(icmp: Arc<icmp::IcmpProber>, snmp: SnmpClient) -> Result<Self, ServerError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(false)
            .build()
            .map_err(|e| ServerError::Probe(format!("failed initializing HTTP client: {e}")))?;

        let dns = hickory_resolver::TokioAsyncResolver::tokio_from_system_conf().ok();

        Ok(Self {
            icmp,
            snmp,
            http,
            dns,
        })
    }
}

impl ProbeExecutor for LocalProbeExecutor {
    fn execute<'a>(
        &'a self,
        target: &'a ProbeTarget,
        probe: &'a v1::ProbeDefinition,
        interface_key: &'a str,
    ) -> BoxFuture<'a, Result<ProbeOutcome, ServerError>> {
        Box::pin(async move {
            network::run_probe(self, target, probe, interface_key).await
        })
    }
}
