//! PULSE: posts, agents, projects, jobs, decide, init, export, remove (port of Go package internal/board).

pub mod agents;
pub mod board;
pub mod commands;
pub mod export;
pub mod focus;
pub mod jobs;
pub mod mentions;
pub mod posts;
pub mod presence;
pub mod projects;
pub mod remove;
pub mod threads;
pub mod write_transaction;

pub use agents::*;
pub use board::*;
pub use commands::*;
pub use export::*;
pub use focus::*;
pub use jobs::*;
pub use mentions::*;
pub use posts::*;
pub use presence::*;
pub use projects::*;
pub use remove::*;
pub use threads::*;
pub use write_transaction::*;

#[cfg(test)]
mod pending_go_parity_tests {
    macro_rules! pending {
        ($($name:ident,)*) => {$(
            #[test]
            #[ignore = "Go board test not yet ported semantically; placeholder keeps the parity gap visible"]
            fn $name() {}
        )*}
    }

    pending! {
        retry_job_in_place_queues_failed_job,
        retry_job_keeps_recover_root_and_increments,
        retry_job_as_new_queues_one_corrected_copy,
        retry_job_refusals_and_cancelled,
        retry_command_output_and_json,
        retry_job_no_source_and_superseded_in_place,
        retry_job_active_run_refused,
        retry_job_source_dedupe_stays_in_project,
        retry_job_cancelled_root_allows_later_child_recovery,
        new_home_with_runner_environment,
        heartbeat_boundaries_return_and_bounded_metadata,
        heartbeat_stop_is_final_and_does_not_extend_last_beat,
        heartbeat_fences_generations_and_historical_sessions,
        heartbeat_missing_malformed_and_ambiguous_evidence,
        heartbeat_restart_idle_legacy_and_pruning_stay_separate,
        prune_agents_keeps_leads,
        heartbeat_concurrent_stop_fences_late_observations,
        mentions_for_go_parity,
        completion_finalize_replaces_branch,
        completion_finalize_rejects_stale_ownership,
        completion_finalize_rolls_back_on_event_failure,
        register_project_runs_the_creation_hook_once,
        register_project_survives_a_failing_hook,
        stop_question_is_the_owners_alone_and_queues_no_job,
        thread_summaries_status_participants_questions_claims_and_handoffs,
        thread_detail_missing_thread_refuses,
        resolve_thread_rules_and_success,
        thread_cli_list_json_and_resolve,
        post_for_owner_cli,
        create_post_for_owner_preserves_extra,
        create_post_rejects_invalid_for_owner,
        project_goal_canonical_slug_registration,
        project_goal_registration_reinit_and_restore,
        project_goal_legacy_registration_preserves_explicit_value,
        remove_project_snapshot,
        remove_executor_snapshot,
        remove_duplicate_findings_go_parity,
        vote_summary_warnings,
        non_vote_summary_warnings,
        rejected_vote_summary_does_not_warn,
        print_post_output_errors,
        agent_focus_leased_job_and_source_thread,
        agent_focus_task_from_source,
        agent_focus_running_run_executor_and_claims,
        agent_focus_claim_only_and_project_filter,
        create_post_summary_limit,
        post_summary_flag_help,
        create_post_rule_matrix,
        create_post_rule_matrix_without_oracle,
        post_id_collision_suffix,
        create_post_mentions_and_events,
        claim_expiry_go_parity,
        vote_command,
        hash_parity_with_oracle,
        helpers_go_parity,
        decide_yes_queues_job,
        decide_no_and_markers,
        owner_away_lead_decide_guards,
        owner_away_lead_decide_still_waits_for_owner,
        lead_yes_job_brief_never_accepts_an_adr,
        jobs_add_finish,
        agents_join_spawn_prune,
        init_registers_project,
        import_registry_go_parity,
        export_store_go_parity,
        board_writes_reach_bus_subscribers,
        decide_is_atomic_under_race,
    }
}
