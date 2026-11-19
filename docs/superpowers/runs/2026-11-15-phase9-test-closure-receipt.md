# Phase9 test closure receipt / 测试闭合回执

This is a running record, not a Phase9 acceptance verdict. / 本文件记录实际运行，不代表 Phase9 验收通过。

The first fresh-target attempt below timed out during dependency compilation; no test binary ran. Its original runner used `Path.with_suffix` on a dotted timestamp, so the memory-gate log names collided with Cargo logs. The gate returned exit 0 before Cargo started, but its original output was overwritten; this attempt cannot substantiate a complete raw memory-gate receipt. The runner now appends log suffixes, with an actual-subprocess regression. The original receipt and compilation output remain preserved. / 下列首轮在依赖编译阶段超时，未执行测试；旧 runner 的时间戳后缀处理覆盖了 memory-gate 日志。启动前 gate 返回 0，但原文已被覆盖，因此此轮不能充当完整内存闸门证据。已修复并保留原始回执。

- 20261116T051923.703983Z: **UNRESOLVED_TIMEOUT**, filter `extension::claude_cache_tests::read_only_discovery_names_a_claude_cache_id_without_loading_it`; receipt `docs/superpowers/runs/phase9-test-output/20261116T051923.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T053403.173012Z: **FAIL**, filter `discovery_fixture_isolates_agents_home_even_when_claude_scanning_is_disabled`; receipt `docs/superpowers/runs/phase9-test-output/20261116T053403.173012Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T054552.622590Z: **FAIL**, filter `discovery_fixture_isolates_agents_home_even_when_claude_scanning_is_disabled`; receipt `docs/superpowers/runs/phase9-test-output/20261116T054552.622590Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T055523.405353Z: **PASS**, filter `discovery_fixture_isolates_agents_home_even_when_claude_scanning_is_disabled`; receipt `docs/superpowers/runs/phase9-test-output/20261116T055523.405353Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060333.584379Z: **FAIL**, filter `extension::claude_cache_tests::a_claude_code_update_moves_the_hook_root_and_the_approval_stays_behind`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060333.584379Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060336.305995Z: **FAIL**, filter `extension::claude_cache_tests::an_unreadable_index_forgets_nothing`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060336.305995Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060337.821944Z: **FAIL**, filter `extension::claude_cache_tests::an_unreadable_index_keeps_the_usage_history`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060337.821944Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060339.344117Z: **PASS**, filter `extension::claude_cache_tests::one_id_in_aleph_home_and_the_claude_cache_is_one_row_the_aleph_copy`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060339.344117Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060340.011778Z: **PASS**, filter `extension::claude_cache_tests::read_only_discovery_names_a_claude_cache_id_without_loading_it`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060340.011778Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060340.613423Z: **FAIL**, filter `extension::claude_cache_tests::the_model_may_disable_but_not_enable_a_claude_cache_row`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060340.613423Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060342.130875Z: **FAIL**, filter `extension::claude_cache_tests::the_rpc_face_enables_and_disables_a_claude_cache_row`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060342.130875Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060343.632497Z: **FAIL**, filter `extension::claude_cache_tests::two_broken_claude_code_plugins_at_one_version_are_two_error_rows`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060343.632497Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060345.297087Z: **FAIL**, filter `extension::context_adapter::tests::lifecycle_mount_dynamic_resolver_grant_revoke_and_replacement`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060345.297087Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060346.813355Z: **PASS**, filter `extension::hooks::consent::tests::only_a_spliced_text_beside_its_literal_sibling_is_superseded`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060346.813355Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060347.415416Z: **PASS**, filter `extension::hooks::executor::tests::a_plugin_hooks_data_directory_reaches_it_through_the_environment`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060347.415416Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060348.020649Z: **PASS**, filter `extension::hooks::output_budget::tests::oversized_context_is_bounded_and_keeps_head_and_tail`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060348.020649Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060348.629238Z: **PASS**, filter `extension::hooks::output_budget::tests::oversized_context_is_recoverable_from_the_spill_file`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060348.629238Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060349.235279Z: **PASS**, filter `extension::hooks::output_budget::tests::preview_is_char_boundary_safe_for_multibyte_text`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060349.235279Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060349.855482Z: **PASS**, filter `extension::hooks::registry::tests::plugin_collection_uses_active_owning_row_identity_despite_builtin_spelling`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060349.855482Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060350.463171Z: **PASS**, filter `extension::hooks::registry::tests::registered_loader_captures_each_real_file_and_preserves_public_configs`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060350.463171Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060351.070770Z: **FAIL**, filter `extension::mcp_plugin_tests::an_absolute_command_outside_the_root_is_counted_and_spawned`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060351.070770Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060352.586893Z: **FAIL**, filter `extension::mcp_plugin_tests::an_explicit_non_mcp_runtime_counts_no_servers_and_says_why`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060352.586893Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060354.104497Z: **FAIL**, filter `extension::mcp_plugin_tests::an_inline_mcp_servers_object_is_counted_and_spawned`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060354.104497Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060355.636410Z: **PASS**, filter `extension::service_ops::tests::manual_service_control_serializes_with_mount_lifecycle`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060355.636410Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060356.995881Z: **PASS**, filter `extension::service_ops::tests::manual_stop_does_not_target_replacement_mount`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060356.995881Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060358.306534Z: **FAIL**, filter `extension::tests::projection_publishes_skill_dirs_with_their_scope_keys`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060358.306534Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060359.851185Z: **PASS**, filter `gateway::capability_events_tests::foreign_scope_denial_records_policy_violation_without_handler_effect`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060359.851185Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060400.466419Z: **PASS**, filter `capability::zahir_facade::tests::stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060400.466419Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060401.064434Z: **PASS**, filter `gateway::capability_events_tests::ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060401.064434Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060401.676024Z: **PASS**, filter `capability::zahir_facade::tests::projection_lag_requires_snapshot_before_reconnect_resolve`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060401.676024Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060402.307412Z: **PASS**, filter `capability::ui_surface_tests::disable_closes_owned_surface_and_rejects_old_lease`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060402.307412Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T060402.913922Z: **PASS**, filter `gateway::capability_terminal_tests::captured_terminal_rejects_provider_replacement_before_invoke`; receipt `docs/superpowers/runs/phase9-test-output/20261116T060402.913922Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T063120.697408Z: **PASS**, filter `discovery_fixture_isolates_agents_home_even_when_claude_scanning_is_disabled`; receipt `docs/superpowers/runs/phase9-test-output/20261116T063120.697408Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064022.347985Z: **PASS**, filter `extension::claude_cache_tests::a_claude_code_update_moves_the_hook_root_and_the_approval_stays_behind`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064022.347985Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064023.917307Z: **PASS**, filter `extension::claude_cache_tests::an_unreadable_index_forgets_nothing`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064023.917307Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064024.551342Z: **PASS**, filter `extension::claude_cache_tests::an_unreadable_index_keeps_the_usage_history`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064024.551342Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064025.202178Z: **PASS**, filter `extension::claude_cache_tests::one_id_in_aleph_home_and_the_claude_cache_is_one_row_the_aleph_copy`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064025.202178Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064025.835847Z: **PASS**, filter `extension::claude_cache_tests::read_only_discovery_names_a_claude_cache_id_without_loading_it`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064025.835847Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064026.435839Z: **PASS**, filter `extension::claude_cache_tests::the_model_may_disable_but_not_enable_a_claude_cache_row`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064026.435839Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064027.075407Z: **PASS**, filter `extension::claude_cache_tests::the_rpc_face_enables_and_disables_a_claude_cache_row`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064027.075407Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064027.713327Z: **PASS**, filter `extension::claude_cache_tests::two_broken_claude_code_plugins_at_one_version_are_two_error_rows`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064027.713327Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064028.703916Z: **PASS**, filter `extension::context_adapter::tests::lifecycle_mount_dynamic_resolver_grant_revoke_and_replacement`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064028.703916Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064029.915120Z: **PASS**, filter `extension::hooks::consent::tests::only_a_spliced_text_beside_its_literal_sibling_is_superseded`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064029.915120Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064030.542515Z: **PASS**, filter `extension::hooks::executor::tests::a_plugin_hooks_data_directory_reaches_it_through_the_environment`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064030.542515Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064031.161161Z: **PASS**, filter `extension::hooks::output_budget::tests::oversized_context_is_bounded_and_keeps_head_and_tail`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064031.161161Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064031.759065Z: **PASS**, filter `extension::hooks::output_budget::tests::oversized_context_is_recoverable_from_the_spill_file`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064031.759065Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064032.355612Z: **PASS**, filter `extension::hooks::output_budget::tests::preview_is_char_boundary_safe_for_multibyte_text`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064032.355612Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064032.956631Z: **PASS**, filter `extension::hooks::registry::tests::plugin_collection_uses_active_owning_row_identity_despite_builtin_spelling`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064032.956631Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064033.543827Z: **PASS**, filter `extension::hooks::registry::tests::registered_loader_captures_each_real_file_and_preserves_public_configs`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064033.543827Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064034.141780Z: **PASS**, filter `extension::mcp_plugin_tests::an_absolute_command_outside_the_root_is_counted_and_spawned`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064034.141780Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064034.812510Z: **PASS**, filter `extension::mcp_plugin_tests::an_explicit_non_mcp_runtime_counts_no_servers_and_says_why`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064034.812510Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064035.423313Z: **PASS**, filter `extension::mcp_plugin_tests::an_inline_mcp_servers_object_is_counted_and_spawned`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064035.423313Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064036.096549Z: **PASS**, filter `extension::service_ops::tests::manual_service_control_serializes_with_mount_lifecycle`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064036.096549Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064037.462506Z: **PASS**, filter `extension::service_ops::tests::manual_stop_does_not_target_replacement_mount`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064037.462506Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064038.763032Z: **PASS**, filter `extension::tests::projection_publishes_skill_dirs_with_their_scope_keys`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064038.763032Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064039.389200Z: **PASS**, filter `gateway::capability_events_tests::foreign_scope_denial_records_policy_violation_without_handler_effect`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064039.389200Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064039.984968Z: **PASS**, filter `capability::zahir_facade::tests::stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064039.984968Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064040.609869Z: **PASS**, filter `gateway::capability_events_tests::ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064040.609869Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064041.232442Z: **PASS**, filter `capability::zahir_facade::tests::projection_lag_requires_snapshot_before_reconnect_resolve`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064041.232442Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064041.890701Z: **PASS**, filter `capability::ui_surface_tests::disable_closes_owned_surface_and_rejects_old_lease`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064041.890701Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064042.502896Z: **PASS**, filter `gateway::capability_terminal_tests::captured_terminal_rejects_provider_replacement_before_invoke`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064042.502896Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064043.118893Z: **PASS**, filter `builtin_tools::code_exec::seatbelt_live_tests::bash_tool_runs_when_path_puts_an_unrunnable_bash_first`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064043.118893Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064043.737474Z: **PASS**, filter `builtin_tools::runtime_manage::tests::a_failing_ledger_still_reports_ok_for_a_real_spawn`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064043.737474Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064044.339274Z: **PASS**, filter `builtin_tools::runtime_manage::tests::install_chromiums_real_subprocess_output_reaches_poll_through_the_tee`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064044.339274Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064047.081078Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::always_reports_chromium_node_and_managed_findings`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064047.081078Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064047.692284Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::never_repairs_in_fix_posture`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064047.692284Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064048.306106Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::prerequisite_findings_are_advisory_never_gating`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064048.306106Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064048.922003Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::the_doctor_claims_a_withheld_family_only_when_the_gate_withholds_it`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064048.922003Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064049.548754Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::the_managed_finding_agrees_with_the_tool_gate`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064049.548754Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064050.177241Z: **PASS**, filter `extension::claude_cache_tests::a_claude_cache_plugin_loads_disabled_and_never_writes_under_claude_home`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064050.177241Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064050.822426Z: **PASS**, filter `extension::mcp_plugin_tests::a_bare_command_is_counted_and_spawned_from_path`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064050.822426Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064051.492015Z: **PASS**, filter `extension::mcp_plugin_tests::a_dot_mcp_json_added_after_the_first_load_is_counted_and_spawned_on_reload`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064051.492015Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064052.161243Z: **PASS**, filter `extension::mcp_plugin_tests::a_relative_or_rooted_command_inside_the_root_is_counted_and_spawned_from_the_root`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064052.161243Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064052.826324Z: **PASS**, filter `extension::mcp_plugin_tests::a_root_variable_that_climbs_out_is_refused_on_both_faces_and_the_row_says_why`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064052.826324Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064053.851538Z: **PASS**, filter `extension::projection::tests::lifecycle_projection_replaces_panel_snapshot_on_replacement_and_disable`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064053.851538Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064055.174776Z: **PASS**, filter `extension::tests::a_disabled_plugin_stays_inactive_across_a_fresh_load`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064055.174776Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064055.810353Z: **PASS**, filter `extension::tests::a_legacy_disabled_marker_is_migrated_then_removed`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064055.810353Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064056.458837Z: **PASS**, filter `extension::tests::plugin_skills_are_owned_by_the_registry_id_not_the_directory_name`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064056.458837Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064057.097638Z: **PASS**, filter `extension::tests::plugin_visible_answers_from_the_rows_key_and_fails_closed_on_unknown_ids`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064057.097638Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064057.783262Z: **PASS**, filter `extension::tests::re_enabling_needs_no_reload`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064057.783262Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064058.475641Z: **PASS**, filter `extension::tests::tool_handler_registry_first_attachment_after_empty_discovery_is_refused`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064058.475641Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064059.128813Z: **PASS**, filter `extension::tests::tool_handler_registry_same_arc_reattachment_is_ok_after_empty_discovery`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064059.128813Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064059.785414Z: **PASS**, filter `sandbox::proxy::netns_bridge::tests::host_bridge_forwards_bytes_uds_to_tcp`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064059.785414Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T064100.502081Z: **FAIL**, filter `FULL LIB`; receipt `docs/superpowers/runs/phase9-test-output/20261116T064100.502081Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T065501.117605Z: **PASS**, filter `extension::claude_cache_tests::a_claude_code_update_moves_the_hook_root_and_the_approval_stays_behind`; receipt `docs/superpowers/runs/phase9-test-output/20261116T065501.117605Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070303.476161Z: **PASS**, filter `extension::claude_cache_tests::an_unreadable_index_forgets_nothing`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070303.476161Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070305.062211Z: **PASS**, filter `extension::claude_cache_tests::an_unreadable_index_keeps_the_usage_history`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070305.062211Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070305.734487Z: **PASS**, filter `extension::claude_cache_tests::one_id_in_aleph_home_and_the_claude_cache_is_one_row_the_aleph_copy`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070305.734487Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070306.382676Z: **PASS**, filter `extension::claude_cache_tests::read_only_discovery_names_a_claude_cache_id_without_loading_it`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070306.382676Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070306.992443Z: **PASS**, filter `extension::claude_cache_tests::the_model_may_disable_but_not_enable_a_claude_cache_row`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070306.992443Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070307.641938Z: **PASS**, filter `extension::claude_cache_tests::the_rpc_face_enables_and_disables_a_claude_cache_row`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070307.641938Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070308.282371Z: **PASS**, filter `extension::claude_cache_tests::two_broken_claude_code_plugins_at_one_version_are_two_error_rows`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070308.282371Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070309.286683Z: **PASS**, filter `extension::context_adapter::tests::lifecycle_mount_dynamic_resolver_grant_revoke_and_replacement`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070309.286683Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070310.516276Z: **PASS**, filter `extension::hooks::consent::tests::only_a_spliced_text_beside_its_literal_sibling_is_superseded`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070310.516276Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070311.126798Z: **PASS**, filter `extension::hooks::executor::tests::a_plugin_hooks_data_directory_reaches_it_through_the_environment`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070311.126798Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070311.736089Z: **PASS**, filter `extension::hooks::output_budget::tests::oversized_context_is_bounded_and_keeps_head_and_tail`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070311.736089Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070312.343238Z: **PASS**, filter `extension::hooks::output_budget::tests::oversized_context_is_recoverable_from_the_spill_file`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070312.343238Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070312.954081Z: **PASS**, filter `extension::hooks::output_budget::tests::preview_is_char_boundary_safe_for_multibyte_text`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070312.954081Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070313.562059Z: **PASS**, filter `extension::hooks::registry::tests::plugin_collection_uses_active_owning_row_identity_despite_builtin_spelling`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070313.562059Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070314.169754Z: **PASS**, filter `extension::hooks::registry::tests::registered_loader_captures_each_real_file_and_preserves_public_configs`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070314.169754Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070314.773636Z: **PASS**, filter `extension::mcp_plugin_tests::an_absolute_command_outside_the_root_is_counted_and_spawned`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070314.773636Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070315.450895Z: **PASS**, filter `extension::mcp_plugin_tests::an_explicit_non_mcp_runtime_counts_no_servers_and_says_why`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070315.450895Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070316.067369Z: **PASS**, filter `extension::mcp_plugin_tests::an_inline_mcp_servers_object_is_counted_and_spawned`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070316.067369Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070316.740595Z: **PASS**, filter `extension::service_ops::tests::manual_service_control_serializes_with_mount_lifecycle`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070316.740595Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070318.128905Z: **PASS**, filter `extension::service_ops::tests::manual_stop_does_not_target_replacement_mount`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070318.128905Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070319.452652Z: **PASS**, filter `extension::tests::projection_publishes_skill_dirs_with_their_scope_keys`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070319.452652Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070320.079346Z: **PASS**, filter `gateway::capability_events_tests::foreign_scope_denial_records_policy_violation_without_handler_effect`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070320.079346Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070320.706388Z: **PASS**, filter `capability::zahir_facade::tests::stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070320.706388Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070321.305771Z: **PASS**, filter `gateway::capability_events_tests::ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070321.305771Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070321.922093Z: **PASS**, filter `capability::zahir_facade::tests::projection_lag_requires_snapshot_before_reconnect_resolve`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070321.922093Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070322.560841Z: **PASS**, filter `capability::ui_surface_tests::disable_closes_owned_surface_and_rejects_old_lease`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070322.560841Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070323.163550Z: **PASS**, filter `gateway::capability_terminal_tests::captured_terminal_rejects_provider_replacement_before_invoke`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070323.163550Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070323.787825Z: **PASS**, filter `builtin_tools::code_exec::seatbelt_live_tests::bash_tool_runs_when_path_puts_an_unrunnable_bash_first`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070323.787825Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070324.411624Z: **PASS**, filter `builtin_tools::runtime_manage::tests::a_failing_ledger_still_reports_ok_for_a_real_spawn`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070324.411624Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070325.013754Z: **FAIL**, filter `builtin_tools::runtime_manage::tests::install_chromiums_real_subprocess_output_reaches_poll_through_the_tee`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070325.013754Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070330.599487Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::always_reports_chromium_node_and_managed_findings`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070330.599487Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070331.220010Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::never_repairs_in_fix_posture`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070331.220010Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070331.831771Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::prerequisite_findings_are_advisory_never_gating`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070331.831771Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070332.440708Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::the_doctor_claims_a_withheld_family_only_when_the_gate_withholds_it`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070332.440708Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070333.061177Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::the_managed_finding_agrees_with_the_tool_gate`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070333.061177Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070333.668187Z: **PASS**, filter `extension::claude_cache_tests::a_claude_cache_plugin_loads_disabled_and_never_writes_under_claude_home`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070333.668187Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070334.305132Z: **PASS**, filter `extension::mcp_plugin_tests::a_bare_command_is_counted_and_spawned_from_path`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070334.305132Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070334.977849Z: **PASS**, filter `extension::mcp_plugin_tests::a_dot_mcp_json_added_after_the_first_load_is_counted_and_spawned_on_reload`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070334.977849Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070335.656009Z: **PASS**, filter `extension::mcp_plugin_tests::a_relative_or_rooted_command_inside_the_root_is_counted_and_spawned_from_the_root`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070335.656009Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070336.325510Z: **PASS**, filter `extension::mcp_plugin_tests::a_root_variable_that_climbs_out_is_refused_on_both_faces_and_the_row_says_why`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070336.325510Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070337.352099Z: **PASS**, filter `extension::projection::tests::lifecycle_projection_replaces_panel_snapshot_on_replacement_and_disable`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070337.352099Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070338.708484Z: **PASS**, filter `extension::tests::a_disabled_plugin_stays_inactive_across_a_fresh_load`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070338.708484Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070339.342466Z: **PASS**, filter `extension::tests::a_legacy_disabled_marker_is_migrated_then_removed`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070339.342466Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070339.964324Z: **PASS**, filter `extension::tests::plugin_skills_are_owned_by_the_registry_id_not_the_directory_name`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070339.964324Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070340.603958Z: **PASS**, filter `extension::tests::plugin_visible_answers_from_the_rows_key_and_fails_closed_on_unknown_ids`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070340.603958Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070341.234271Z: **PASS**, filter `extension::tests::re_enabling_needs_no_reload`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070341.234271Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070341.869160Z: **PASS**, filter `extension::tests::tool_handler_registry_first_attachment_after_empty_discovery_is_refused`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070341.869160Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070342.488309Z: **PASS**, filter `extension::tests::tool_handler_registry_same_arc_reattachment_is_ok_after_empty_discovery`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070342.488309Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070343.105870Z: **PASS**, filter `sandbox::proxy::netns_bridge::tests::host_bridge_forwards_bytes_uds_to_tcp`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070343.105870Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070343.770737Z: **PASS**, filter `extension::visibility::tests::test_scope_install_dir_user`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070343.770737Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070344.376628Z: **PASS**, filter `gateway::handlers::daemon_control::tests::log_directory_is_under_home`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070344.376628Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070344.976620Z: **PASS**, filter `utils::paths::tests::discovery_fixture_isolates_agents_home_even_when_claude_scanning_is_disabled`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070344.976620Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T070345.591159Z: **PASS**, filter `FULL LIB`; receipt `docs/superpowers/runs/phase9-test-output/20261116T070345.591159Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T074454.402127Z: **FAIL**, filter `extension::claude_cache_tests::a_claude_code_update_moves_the_hook_root_and_the_approval_stays_behind`; receipt `docs/superpowers/runs/phase9-test-output/20261116T074454.402127Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T074816.774635Z: **FAIL**, filter `extension::claude_cache_tests::an_unreadable_index_forgets_nothing`; receipt `docs/superpowers/runs/phase9-test-output/20261116T074816.774635Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T075132.622410Z: **FAIL**, filter `extension::claude_cache_tests::an_unreadable_index_keeps_the_usage_history`; receipt `docs/superpowers/runs/phase9-test-output/20261116T075132.622410Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T075452.939924Z: **FAIL**, filter `extension::claude_cache_tests::one_id_in_aleph_home_and_the_claude_cache_is_one_row_the_aleph_copy`; receipt `docs/superpowers/runs/phase9-test-output/20261116T075452.939924Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T075813.343135Z: **FAIL**, filter `extension::claude_cache_tests::read_only_discovery_names_a_claude_cache_id_without_loading_it`; receipt `docs/superpowers/runs/phase9-test-output/20261116T075813.343135Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T080747.835140Z: **PASS**, filter `builtin_tools::runtime_manage::tests::install_chromiums_real_subprocess_output_reaches_poll_through_the_tee`; receipt `docs/superpowers/runs/phase9-test-output/20261116T080747.835140Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081553.610963Z: **PASS**, filter `extension::claude_cache_tests::a_claude_code_update_moves_the_hook_root_and_the_approval_stays_behind`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081553.610963Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081555.238639Z: **PASS**, filter `extension::claude_cache_tests::an_unreadable_index_forgets_nothing`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081555.238639Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081555.886565Z: **PASS**, filter `extension::claude_cache_tests::an_unreadable_index_keeps_the_usage_history`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081555.886565Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081556.556517Z: **PASS**, filter `extension::claude_cache_tests::one_id_in_aleph_home_and_the_claude_cache_is_one_row_the_aleph_copy`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081556.556517Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081557.201610Z: **PASS**, filter `extension::claude_cache_tests::read_only_discovery_names_a_claude_cache_id_without_loading_it`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081557.201610Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081557.811353Z: **PASS**, filter `extension::claude_cache_tests::the_model_may_disable_but_not_enable_a_claude_cache_row`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081557.811353Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081558.463605Z: **PASS**, filter `extension::claude_cache_tests::the_rpc_face_enables_and_disables_a_claude_cache_row`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081558.463605Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081559.111326Z: **PASS**, filter `extension::claude_cache_tests::two_broken_claude_code_plugins_at_one_version_are_two_error_rows`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081559.111326Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081600.130682Z: **PASS**, filter `extension::context_adapter::tests::lifecycle_mount_dynamic_resolver_grant_revoke_and_replacement`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081600.130682Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081601.294617Z: **PASS**, filter `extension::hooks::consent::tests::only_a_spliced_text_beside_its_literal_sibling_is_superseded`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081601.294617Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081601.915862Z: **PASS**, filter `extension::hooks::executor::tests::a_plugin_hooks_data_directory_reaches_it_through_the_environment`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081601.915862Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081602.536640Z: **PASS**, filter `extension::hooks::output_budget::tests::oversized_context_is_bounded_and_keeps_head_and_tail`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081602.536640Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081603.155229Z: **PASS**, filter `extension::hooks::output_budget::tests::oversized_context_is_recoverable_from_the_spill_file`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081603.155229Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081603.773944Z: **PASS**, filter `extension::hooks::output_budget::tests::preview_is_char_boundary_safe_for_multibyte_text`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081603.773944Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081604.389603Z: **PASS**, filter `extension::hooks::registry::tests::plugin_collection_uses_active_owning_row_identity_despite_builtin_spelling`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081604.389603Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081605.004052Z: **PASS**, filter `extension::hooks::registry::tests::registered_loader_captures_each_real_file_and_preserves_public_configs`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081605.004052Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081605.621077Z: **PASS**, filter `extension::mcp_plugin_tests::an_absolute_command_outside_the_root_is_counted_and_spawned`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081605.621077Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081606.304766Z: **PASS**, filter `extension::mcp_plugin_tests::an_explicit_non_mcp_runtime_counts_no_servers_and_says_why`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081606.304766Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081606.939919Z: **PASS**, filter `extension::mcp_plugin_tests::an_inline_mcp_servers_object_is_counted_and_spawned`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081606.939919Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081607.625186Z: **PASS**, filter `extension::service_ops::tests::manual_service_control_serializes_with_mount_lifecycle`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081607.625186Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081608.878836Z: **PASS**, filter `extension::service_ops::tests::manual_stop_does_not_target_replacement_mount`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081608.878836Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081610.091733Z: **PASS**, filter `extension::tests::projection_publishes_skill_dirs_with_their_scope_keys`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081610.091733Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081610.731835Z: **PASS**, filter `gateway::capability_events_tests::foreign_scope_denial_records_policy_violation_without_handler_effect`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081610.731835Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081611.361403Z: **PASS**, filter `capability::zahir_facade::tests::stale_lease_after_owner_bump_fails_and_fresh_resolve_succeeds`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081611.361403Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081611.987454Z: **PASS**, filter `gateway::capability_events_tests::ui_surface_cancel_after_dispatch_joins_worker_and_clears_pending`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081611.987454Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081612.619809Z: **PASS**, filter `capability::zahir_facade::tests::projection_lag_requires_snapshot_before_reconnect_resolve`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081612.619809Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081613.271667Z: **PASS**, filter `capability::ui_surface_tests::disable_closes_owned_surface_and_rejects_old_lease`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081613.271667Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081613.887794Z: **PASS**, filter `gateway::capability_terminal_tests::captured_terminal_rejects_provider_replacement_before_invoke`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081613.887794Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081614.517069Z: **PASS**, filter `builtin_tools::code_exec::seatbelt_live_tests::bash_tool_runs_when_path_puts_an_unrunnable_bash_first`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081614.517069Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081615.149435Z: **PASS**, filter `builtin_tools::runtime_manage::tests::a_failing_ledger_still_reports_ok_for_a_real_spawn`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081615.149435Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081615.761382Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::always_reports_chromium_node_and_managed_findings`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081615.761382Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081616.391940Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::never_repairs_in_fix_posture`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081616.391940Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081617.023340Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::prerequisite_findings_are_advisory_never_gating`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081617.023340Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081617.647532Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::the_doctor_claims_a_withheld_family_only_when_the_gate_withholds_it`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081617.647532Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081618.269981Z: **PASS**, filter `diagnostics::checks::browser_runtime::tests::the_managed_finding_agrees_with_the_tool_gate`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081618.269981Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081618.892296Z: **PASS**, filter `extension::claude_cache_tests::a_claude_cache_plugin_loads_disabled_and_never_writes_under_claude_home`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081618.892296Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081619.534572Z: **PASS**, filter `extension::mcp_plugin_tests::a_bare_command_is_counted_and_spawned_from_path`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081619.534572Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081620.213214Z: **PASS**, filter `extension::mcp_plugin_tests::a_dot_mcp_json_added_after_the_first_load_is_counted_and_spawned_on_reload`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081620.213214Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081620.894752Z: **PASS**, filter `extension::mcp_plugin_tests::a_relative_or_rooted_command_inside_the_root_is_counted_and_spawned_from_the_root`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081620.894752Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081621.598641Z: **PASS**, filter `extension::mcp_plugin_tests::a_root_variable_that_climbs_out_is_refused_on_both_faces_and_the_row_says_why`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081621.598641Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081622.576344Z: **PASS**, filter `extension::projection::tests::lifecycle_projection_replaces_panel_snapshot_on_replacement_and_disable`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081622.576344Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081623.944448Z: **PASS**, filter `extension::tests::a_disabled_plugin_stays_inactive_across_a_fresh_load`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081623.944448Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081624.590259Z: **PASS**, filter `extension::tests::a_legacy_disabled_marker_is_migrated_then_removed`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081624.590259Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081625.221178Z: **PASS**, filter `extension::tests::plugin_skills_are_owned_by_the_registry_id_not_the_directory_name`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081625.221178Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081625.858508Z: **PASS**, filter `extension::tests::plugin_visible_answers_from_the_rows_key_and_fails_closed_on_unknown_ids`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081625.858508Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081626.488986Z: **PASS**, filter `extension::tests::re_enabling_needs_no_reload`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081626.488986Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081627.130746Z: **PASS**, filter `extension::tests::tool_handler_registry_first_attachment_after_empty_discovery_is_refused`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081627.130746Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081627.763786Z: **PASS**, filter `extension::tests::tool_handler_registry_same_arc_reattachment_is_ok_after_empty_discovery`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081627.763786Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081628.397315Z: **PASS**, filter `sandbox::proxy::netns_bridge::tests::host_bridge_forwards_bytes_uds_to_tcp`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081628.397315Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081629.071437Z: **PASS**, filter `extension::visibility::tests::test_scope_install_dir_user`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081629.071437Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081629.683527Z: **PASS**, filter `gateway::handlers::daemon_control::tests::log_directory_is_under_home`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081629.683527Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081630.298626Z: **PASS**, filter `utils::paths::tests::discovery_fixture_isolates_agents_home_even_when_claude_scanning_is_disabled`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081630.298626Z.json`. / 原始输出与退出码见 JSON 回执。

- 20261116T081630.936679Z: **PASS**, filter `FULL LIB`; receipt `docs/superpowers/runs/phase9-test-output/20261116T081630.936679Z.json`. / 原始输出与退出码见 JSON 回执。
