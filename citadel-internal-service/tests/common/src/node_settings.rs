//! The settings every test node requires (the SDK builds no node without them), set as the
//! agent binary and the workspace server set them, except that the maximum level is the SDK's
//! test one.

use citadel_internal_service::kernel::revfs_seed::{self, MemoryRevfsVault};
use citadel_internal_service::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_sdk::prelude::{BackendType, LinkManagerMode, NodeBuilder, Ratchet};
use std::sync::Arc;

pub trait TestNode<R: Ratchet> {
    /// An agent node: also `kernel`'s RE-VFS keys, sealed under a vault in memory (the test
    /// agents' accounts do not outlive the test).
    fn as_test_agent<T: IOInterface>(
        &mut self,
        kernel: &CitadelWorkspaceService<T, R>,
    ) -> &mut Self;
    /// An agent node whose vault follows `backend` as the agent binary's does
    /// (`revfs_seed::vault_for`), for a test that restarts the agent over the same store.
    fn as_restartable_test_agent<T: IOInterface>(
        &mut self,
        kernel: &CitadelWorkspaceService<T, R>,
        backend: &BackendType,
    ) -> &mut Self;
    /// Any other node: a server, which stores RE-VFS objects as opaque bytes and needs no key,
    /// or a client of the SDK's own kernels.
    fn as_test_node(&mut self) -> &mut Self;
}

impl<R: Ratchet> TestNode<R> for NodeBuilder<R> {
    fn as_test_agent<T: IOInterface>(
        &mut self,
        kernel: &CitadelWorkspaceService<T, R>,
    ) -> &mut Self {
        let keys = kernel
            .revfs_key_source(Arc::new(MemoryRevfsVault::default()))
            .expect("a test agent is given one vault");
        self.as_test_node().with_revfs_key_source(keys)
    }

    fn as_restartable_test_agent<T: IOInterface>(
        &mut self,
        kernel: &CitadelWorkspaceService<T, R>,
        backend: &BackendType,
    ) -> &mut Self {
        let vault = revfs_seed::vault_for(backend).expect("a test backend has a vault");
        let keys = kernel
            .revfs_key_source(vault)
            .expect("a test agent is given one vault");
        self.as_test_node().with_revfs_key_source(keys)
    }

    fn as_test_node(&mut self) -> &mut Self {
        self.with_max_security_level(citadel_sdk::test_common::localhost_max_security_level())
            .with_link_manager(LinkManagerMode::Legacy)
    }
}
