//! Typed data-tool adapters. Only the runtime crosses a long-lived boundary.

use std::marker::PhantomData;

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::runtime::{MemoryOperation, MemoryRuntime};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[async_trait]
pub(crate) trait MemoryToolFactory: AlephTool {
    async fn for_operation(operation: &MemoryOperation) -> Result<Self, AlephError>;
}

#[derive(Clone)]
pub(crate) struct RuntimeTool<T> {
    runtime: Arc<MemoryRuntime>,
    tool: PhantomData<fn() -> T>,
}

impl<T> RuntimeTool<T> {
    pub(crate) fn new(runtime: Arc<MemoryRuntime>) -> Self {
        Self {
            runtime,
            tool: PhantomData,
        }
    }
}

impl RuntimeTool<super::remember::RememberTool> {
    pub(crate) async fn call_for_agent(
        &self,
        agent_id: &str,
        args: super::remember::RememberArgs,
    ) -> Result<super::remember::RememberOutput, AlephError> {
        let operation = self.runtime.operation()?;
        let captured = operation.clone();
        let agent = agent_id.to_owned();
        operation
            .run(async move {
                let services = captured.services()?;
                let store = services.context.get_or_load_curated_store(&agent).await?;
                super::remember::RememberTool::new(store)
                    .with_decision_log(Some(captured.backend().clone()))
                    .call(args)
                    .await
            })
            .await
    }
}

impl RuntimeTool<super::note_orient::NoteOrientTool> {
    pub(crate) async fn call(
        &self,
        agent_id: &str,
        args: super::note_orient::NoteOrientArgs,
    ) -> Result<super::note_orient::NoteOrientOutput, AlephError> {
        let operation = self.runtime.operation()?;
        let services = operation.services()?;
        let agent = agent_id.to_owned();
        operation
            .run(async move {
                super::note_orient::NoteOrientTool::new(
                    services.orientation.clone(),
                    crate::memory::notes::orientation::types::TokenBudget {
                        max_tokens: services.config.orientation.max_tokens,
                    },
                )
                .call(&agent, args)
                .await
            })
            .await
    }
}

impl RuntimeTool<super::note_schema::NoteSchemaTool> {
    pub(crate) async fn call(
        &self,
        agent_id: &str,
        args: super::note_schema::NoteSchemaArgs,
    ) -> Result<super::note_schema::NoteSchemaOutput, AlephError> {
        let operation = self.runtime.operation()?;
        let services = operation.services()?;
        let agent = agent_id.to_owned();
        operation
            .run(async move {
                super::note_schema::NoteSchemaTool::new(services.note_memory_dir.clone())
                    .call(&agent, args)
                    .await
            })
            .await
    }
}

impl RuntimeTool<super::user_profile::UserProfileTool> {
    pub(crate) async fn call(
        &self,
        agent_id: &str,
        args: super::user_profile::UserProfileArgs,
    ) -> Result<super::user_profile::UserProfileOutput, AlephError> {
        let operation = self.runtime.operation()?;
        let services = operation.services()?;
        let profile = services
            .profile
            .clone()
            .ok_or_else(|| AlephError::other("memory user profile is unavailable"))?;
        let agent = agent_id.to_owned();
        operation
            .run(async move {
                super::user_profile::UserProfileTool::new(profile)
                    .call(&agent, args)
                    .await
            })
            .await
    }
}

impl RuntimeTool<super::note_graph_query::NoteGraphQueryTool> {
    pub(crate) async fn call_impl(
        &self,
        agent_id: &str,
        args: super::note_graph_query::NoteGraphQueryArgs,
    ) -> Result<super::note_graph_query::NoteGraphQueryResult, AlephError> {
        let operation = self.runtime.operation()?;
        let backend = operation.backend().clone();
        let agent = agent_id.to_owned();
        operation
            .run(async move {
                super::note_graph_query::NoteGraphQueryTool::new(backend, agent)
                    .call_impl(args)
                    .await
                    .map_err(|error| AlephError::other(error.to_string()))
            })
            .await
    }
}

impl RuntimeTool<super::memory_trace::MemoryTraceTool> {
    pub(crate) async fn call_impl(
        &self,
        agent_id: &str,
        args: super::memory_trace::MemoryTraceArgs,
    ) -> Result<super::memory_trace::TraceResult, AlephError> {
        let operation = self.runtime.operation()?;
        let services = operation.services()?;
        let backend = operation.backend().clone();
        let agent = agent_id.to_owned();
        operation
            .run(async move {
                super::memory_trace::MemoryTraceTool::new(
                    backend,
                    services.raw_store.clone(),
                    agent,
                    services.note_memory_dir.clone(),
                )
                .call_impl(args)
                .await
                .map_err(|error| AlephError::other(error.to_string()))
            })
            .await
    }
}

#[async_trait]
impl<T> AlephTool for RuntimeTool<T>
where
    T: MemoryToolFactory,
    T::Args: 'static,
    T::Output: 'static,
{
    const NAME: &'static str = T::NAME;
    const DESCRIPTION: &'static str = T::DESCRIPTION;
    const MAX_RESULT_TOKENS: Option<usize> = T::MAX_RESULT_TOKENS;
    type Args = T::Args;
    type Output = T::Output;

    fn services() -> Vec<aleph_slot_api::capability::Service> {
        T::services()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, AlephError> {
        let operation = self.runtime.operation()?;
        let captured = operation.clone();
        operation
            .run(async move { T::for_operation(&captured).await?.call(args).await })
            .await
    }
}

#[async_trait]
impl MemoryToolFactory for super::note_manage::NoteManageTool {
    async fn for_operation(operation: &MemoryOperation) -> Result<Self, AlephError> {
        let services = operation.services()?;
        let mut tool = Self::new(
            services.note_memory_dir.clone(),
            operation.backend().clone(),
        )
        .with_project_scoping(services.config.project_scoped);
        if let Some(database) = &services.state_db {
            tool = tool.with_command_handler(Arc::new(
                crate::memory::events::handler::MemoryCommandHandler::new(database.clone()),
            ));
        }
        if let Some(embedder) = services.embedding.get_active_provider().await {
            tool = tool.with_embedder(embedder);
        }
        Ok(tool)
    }
}

#[async_trait]
impl MemoryToolFactory for super::remember::RememberTool {
    async fn for_operation(operation: &MemoryOperation) -> Result<Self, AlephError> {
        let services = operation.services()?;
        let agent = crate::tools::turn_context::current_agent_id()
            .unwrap_or_else(|| services.default_agent_id.clone());
        let store = services.context.get_or_load_curated_store(&agent).await?;
        Ok(Self::new(store).with_decision_log(Some(operation.backend().clone())))
    }
}

#[async_trait]
impl MemoryToolFactory for super::memory_browse::MemoryBrowseTool {
    async fn for_operation(operation: &MemoryOperation) -> Result<Self, AlephError> {
        let services = operation.services()?;
        Ok(Self::new(
            services.note_memory_dir.clone(),
            services.default_agent_id.clone(),
        )
        .with_project_scoping(services.config.project_scoped))
    }
}

#[async_trait]
impl MemoryToolFactory for super::memory_explore::MemoryExploreTool {
    async fn for_operation(operation: &MemoryOperation) -> Result<Self, AlephError> {
        let services = operation.services()?;
        Ok(Self::with_agent_id(
            operation.backend().clone(),
            services.embedding.require_active_provider().await?,
            services.default_agent_id.clone(),
        )
        .with_project_scoping(services.config.project_scoped))
    }
}

#[async_trait]
impl MemoryToolFactory for super::memory_search::MemorySearchTool {
    async fn for_operation(operation: &MemoryOperation) -> Result<Self, AlephError> {
        let services = operation.services()?;
        let retrieval = match services.embedding.get_active_provider().await {
            Some(embedder) => crate::memory::note_retrieval::NoteFactRetrieval::new(
                services.indexer.clone(),
                embedder,
            ),
            None => crate::memory::note_retrieval::NoteFactRetrieval::new_fts_only(
                services.indexer.clone(),
            ),
        }
        .with_rerank_config(&services.config.rerank)
        .with_scoring_config(&services.config.retrieval_scoring)
        .with_expansion_config(&services.config.expansion);
        Ok(
            Self::from_retrieval(services.raw_store.clone(), Arc::new(retrieval))
                .with_project_scoping(services.config.project_scoped),
        )
    }
}
