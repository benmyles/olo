//! The Token Pool's program for Rex hosts, as the app carries it
//! (`build.rs`): one for each kind of host, and the name of the build.

use olo_token_pool::hosts::Build;

pub const ID: &str = env!("OLO_POOL_HOST_ID");

pub fn builds() -> Vec<Build> {
    vec![Build { machines: &["x86_64", "amd64"], bytes: include_bytes!(env!("OLO_POOL_HOST_X86_64")) }, Build { machines: &["aarch64", "arm64"], bytes: include_bytes!(env!("OLO_POOL_HOST_AARCH64")) }]
}
