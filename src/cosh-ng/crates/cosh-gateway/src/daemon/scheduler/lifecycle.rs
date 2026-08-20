impl<F: RuntimeFactory> TaskScheduler<F> {
    /// Opens the durable Task database with an injected Runtime factory.
    ///
    /// # Errors
    ///
    /// Returns a storage or installation-identity error when durable state
    /// cannot be opened safely.
    pub fn open(
        database_path: impl AsRef<Path>,
        requested_installation_id: Option<InstallationId>,
        worker_id: BoundedOpaque,
        factory: F,
    ) -> Result<Self, GatewayDaemonError> {
        Self::open_for_capability_profile(
            database_path,
            requested_installation_id,
            worker_id,
            GatewayCapabilityProfile::task_only_v1(),
            factory,
        )
    }

    /// Opens durable scheduling bound to one trusted capability profile.
    pub(crate) fn open_for_capability_profile(
        database_path: impl AsRef<Path>,
        requested_installation_id: Option<InstallationId>,
        worker_id: BoundedOpaque,
        expected_profile: GatewayCapabilityProfile,
        factory: F,
    ) -> Result<Self, GatewayDaemonError> {
        Self::open_with_profile_and_config(
            database_path,
            requested_installation_id,
            worker_id,
            expected_profile,
            factory,
            TaskSchedulerConfig::default(),
        )
    }

    /// Opens durable state with explicit, validated lease timing bounds.
    pub fn open_with_config(
        database_path: impl AsRef<Path>,
        requested_installation_id: Option<InstallationId>,
        worker_id: BoundedOpaque,
        factory: F,
        config: TaskSchedulerConfig,
    ) -> Result<Self, GatewayDaemonError> {
        Self::open_with_profile_and_config(
            database_path,
            requested_installation_id,
            worker_id,
            GatewayCapabilityProfile::task_only_v1(),
            factory,
            config,
        )
    }

    fn open_with_profile_and_config(
        database_path: impl AsRef<Path>,
        requested_installation_id: Option<InstallationId>,
        worker_id: BoundedOpaque,
        expected_profile: GatewayCapabilityProfile,
        factory: F,
        config: TaskSchedulerConfig,
    ) -> Result<Self, GatewayDaemonError> {
        Ok(Self {
            coordinator: TaskCoordinator::open_for_capability_profile(
                database_path,
                requested_installation_id,
                expected_profile,
            )?,
            worker_id,
            config: config.validate()?,
            factory,
            brokered_driver: Box::new(RejectingBrokeredExecutionDriver),
            active: None,
            shutting_down: false,
            #[cfg(test)]
            fail_next_brokered_result_completion: false,
            #[cfg(test)]
            fail_next_terminal_lease_release: false,
            #[cfg(test)]
            fail_next_input_dispatch_completion: false,
            #[cfg(test)]
            fail_next_input_request_install: false,
            #[cfg(test)]
            fail_next_input_unknown_cleanup: false,
        })
    }

    /// Installs the trusted brokered policy and execution boundary.
    ///
    /// The default driver rejects every brokered request, so callers must
    /// explicitly install a production driver before selecting that profile.
    pub fn with_brokered_execution_driver(
        mut self,
        driver: Box<dyn BrokeredExecutionDriver>,
    ) -> Self {
        self.brokered_driver = driver;
        self
    }

    #[cfg(test)]
    pub(super) fn fail_next_terminal_lease_release_for_test(&mut self) {
        self.fail_next_terminal_lease_release = true;
    }

    #[cfg(test)]
    fn fail_next_input_dispatch_completion_for_test(&mut self) {
        self.fail_next_input_dispatch_completion = true;
    }

    #[cfg(test)]
    fn fail_next_input_request_install_for_test(&mut self) {
        self.fail_next_input_request_install = true;
    }

    #[cfg(test)]
    fn fail_next_input_unknown_cleanup_for_test(&mut self) {
        self.fail_next_input_unknown_cleanup = true;
    }

}
