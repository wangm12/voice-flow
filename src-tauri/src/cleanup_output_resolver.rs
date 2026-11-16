//! Dormant, typed output-mode resolution for cleanup-input projection.
//!
//! The Stage 2 resolver is testable independently from provider builders. Its
//! result is an ephemeral view and never mutates canonical preparation.

use crate::cleanup_output_layout::{
    project_cleanup_layout, CleanupLayoutProjection, OutputLayoutMode, ProjectedStructure,
    ProjectionError,
};
use crate::cleanup_prompt::{
    AutoScene, EditBudget, EffectiveCleanupPlan, OutputMode, StructureClass,
};
use crate::context::{ContextFamily, FocusKind};
use crate::llm::{CleanupIntensity, CleanupOperation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputOperation {
    OrdinaryCleanup,
    AuthorizedTranslation,
    Other,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CleanupOutputFacts {
    pub(crate) intensity: CleanupIntensity,
    pub(crate) output_mode: OutputMode,
    pub(crate) auto_scene: AutoScene,
    pub(crate) operation: OutputOperation,
    /// Existing route/consent/fallback decisions are authoritative.
    pub(crate) model_request_eligible: bool,
    /// The target can accept a compatible output-mode presentation.
    pub(crate) output_mode_compatible: bool,
    /// Captured from the existing native Accessibility role. This gates only
    /// inferred layout; the coarse FocusKind is not sufficient evidence.
    pub(crate) multiline_capable: bool,
    pub(crate) focus_kind: FocusKind,
    /// A consented task preference may select bullets, but projection still
    /// requires explicit source boundaries and the existing target checks.
    pub(crate) preferred_bullets: bool,
    /// The live caller has positively established an ordinary Auto prose
    /// request that may ask the model to choose paragraph breaks.
    pub(crate) allow_model_selected_paragraphs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCleanupProjection {
    pub(crate) plan: EffectiveCleanupPlan,
    pub(crate) projection: CleanupLayoutProjection,
    pub(crate) operation: CleanupOperation,
    pub(crate) requested_layout: OutputLayoutMode,
}

/// Resolve one effective plan and one source-offset-only projection without
/// changing canonical prepared text or its authorization bindings.
pub(crate) fn resolve_cleanup_projection(
    facts: CleanupOutputFacts,
    canonical_projection_source: &str,
) -> Result<Option<ResolvedCleanupProjection>, ProjectionError> {
    if !facts.model_request_eligible {
        return Ok(None);
    }
    if facts.output_mode == OutputMode::Code {
        return Ok(None);
    }

    let operation = match facts.operation {
        OutputOperation::OrdinaryCleanup => CleanupOperation::Cleanup,
        OutputOperation::AuthorizedTranslation => CleanupOperation::Translate,
        OutputOperation::Other => return Ok(None),
    };
    if facts.output_mode == OutputMode::Translation && operation != CleanupOperation::Translate {
        return Ok(None);
    }

    let Some(edit_budget) = edit_budget_for_resolved_intensity(facts.intensity) else {
        return Ok(None);
    };
    if edit_budget == EditBudget::Off {
        return Ok(None);
    }

    // A manual format is compatible only after the caller has established a
    // prose target. Layout evidence is evaluated separately, so weak source
    // evidence can leave prose intact while retaining its one selected add-on.
    let preference_requests_bullets = facts.preferred_bullets
        && facts.output_mode == OutputMode::Auto
        && facts.output_mode_compatible
        && operation == CleanupOperation::Cleanup;
    let candidate_mode = match (facts.output_mode, operation) {
        (_, CleanupOperation::Translate) => OutputMode::Translation,
        (OutputMode::Auto, _) | (OutputMode::Code, _) | (OutputMode::Translation, _) => {
            facts.output_mode
        }
        (OutputMode::Email | OutputMode::Bullets | OutputMode::MeetingNotes, _)
            if facts.output_mode_compatible =>
        {
            facts.output_mode
        }
        (OutputMode::Email | OutputMode::Bullets | OutputMode::MeetingNotes, _) => OutputMode::Auto,
    };

    let mut requested_layout = if operation == CleanupOperation::Cleanup
        && facts.output_mode_compatible
        && can_project_layout(facts.focus_kind, facts.multiline_capable)
    {
        if preference_requests_bullets {
            OutputLayoutMode::List
        } else {
            layout_mode(candidate_mode, facts.auto_scene)
        }
    } else {
        OutputLayoutMode::None
    };
    let mut projection = project_cleanup_layout(canonical_projection_source, requested_layout)?;
    let mut preference_selected =
        preference_requests_bullets && projection.structure == ProjectedStructure::List;
    if preference_requests_bullets
        && requested_layout == OutputLayoutMode::List
        && !preference_selected
    {
        // If the source cannot prove a list, resolve the original mode's
        // layout from the same canonical text instead of keeping a failed
        // preference projection. This preserves AutoEmail paragraph breaks.
        requested_layout = layout_mode(candidate_mode, facts.auto_scene);
        projection = project_cleanup_layout(canonical_projection_source, requested_layout)?;
        preference_selected = false;
    }
    let selected_mode = if preference_requests_bullets && preference_selected {
        OutputMode::Bullets
    } else {
        candidate_mode
    };
    let structure = if crate::spoken_layout::has_structural_layout(canonical_projection_source) {
        StructureClass::SpokenOrRendered
    } else if projection.structure != ProjectedStructure::Unchanged {
        StructureClass::SourceBackedInference
    } else if facts.allow_model_selected_paragraphs
        && facts.output_mode == OutputMode::Auto
        && operation == CleanupOperation::Cleanup
        && facts.output_mode_compatible
        && !matches!(
            facts.focus_kind,
            FocusKind::Unknown | FocusKind::Code | FocusKind::Search
        )
        && can_project_layout(facts.focus_kind, facts.multiline_capable)
    {
        StructureClass::ModelSelectedParagraphs
    } else {
        StructureClass::None
    };
    let Some(mut plan) = EffectiveCleanupPlan::for_operation(
        edit_budget,
        selected_mode,
        facts.auto_scene,
        operation,
        structure,
    ) else {
        return Ok(None);
    };
    if preference_selected {
        plan.mode_source = crate::cleanup_prompt::ModeSource::Preference;
    }

    Ok(Some(ResolvedCleanupProjection {
        plan,
        projection,
        operation,
        requested_layout,
    }))
}

fn edit_budget_for_resolved_intensity(intensity: CleanupIntensity) -> Option<EditBudget> {
    match intensity {
        CleanupIntensity::Off => Some(EditBudget::Off),
        CleanupIntensity::Light => Some(EditBudget::Light),
        CleanupIntensity::Standard => Some(EditBudget::Standard),
        CleanupIntensity::Heavy => Some(EditBudget::Heavy),
        // Callers must first resolve Auto through the existing scene and App
        // mapping precedence owner.
        CleanupIntensity::Auto => None,
    }
}

fn layout_mode(mode: OutputMode, auto_scene: AutoScene) -> OutputLayoutMode {
    match mode {
        OutputMode::Email => OutputLayoutMode::Email,
        OutputMode::Bullets => OutputLayoutMode::List,
        OutputMode::MeetingNotes => OutputLayoutMode::Paragraph,
        OutputMode::Auto if auto_scene == AutoScene::Email => OutputLayoutMode::Paragraph,
        OutputMode::Auto | OutputMode::Code | OutputMode::Translation => OutputLayoutMode::None,
    }
}

pub(crate) fn auto_scene_for_context(
    profile_id: &str,
    family: ContextFamily,
    effective_input_kind: FocusKind,
) -> AutoScene {
    if matches!(
        family,
        ContextFamily::PromptOrCode | ContextFamily::DeveloperCollaboration
    ) && effective_input_kind == FocusKind::CodingPrompt
    {
        return AutoScene::DeveloperPrompt;
    }
    if profile_id == "developer.web"
        && family == ContextFamily::Document
        && effective_input_kind == FocusKind::Document
    {
        return AutoScene::IssueBody;
    }
    match family {
        ContextFamily::Email => AutoScene::Email,
        ContextFamily::PersonalChat => AutoScene::PersonalChat,
        ContextFamily::WorkChat => AutoScene::WorkChat,
        ContextFamily::Document => AutoScene::Document,
        ContextFamily::NotesJournaling => AutoScene::Notes,
        _ => AutoScene::General,
    }
}

/// An explicit layout capability is carried from the observed field. FocusKind
/// remains useful for route/scene policy, but does not prove multiline input.
pub(crate) fn can_project_layout(
    focus_kind: FocusKind,
    captured_multiline_capability: bool,
) -> bool {
    captured_multiline_capability
        && !matches!(
            focus_kind,
            FocusKind::Secure | FocusKind::Terminal | FocusKind::Form
        )
}

#[cfg(test)]
mod tests {
    use super::{
        auto_scene_for_context, can_project_layout, resolve_cleanup_projection, CleanupOutputFacts,
        OutputOperation,
    };
    use crate::cleanup_output_layout::{OutputLayoutMode, ProjectedStructure};
    use crate::cleanup_prompt::{
        AutoScene, EditBudget, ModeSource, OutputAddOn, OutputMode, StructureClass,
    };
    use crate::context::{ContextFamily, FocusKind};
    use crate::llm::CleanupIntensity;

    fn facts(
        mode: OutputMode,
        scene: AutoScene,
        intensity: CleanupIntensity,
    ) -> CleanupOutputFacts {
        CleanupOutputFacts {
            intensity,
            output_mode: mode,
            auto_scene: scene,
            operation: if mode == OutputMode::Translation {
                OutputOperation::AuthorizedTranslation
            } else {
                OutputOperation::OrdinaryCleanup
            },
            model_request_eligible: true,
            output_mode_compatible: true,
            multiline_capable: true,
            focus_kind: FocusKind::Document,
            preferred_bullets: false,
            allow_model_selected_paragraphs: false,
        }
    }

    #[test]
    fn manual_mode_selects_one_add_on_even_when_source_evidence_keeps_prose() {
        let source = "Please review this note, then tell me whether you agree.";
        let result = resolve_cleanup_projection(
            facts(
                OutputMode::Bullets,
                AutoScene::WorkChat,
                CleanupIntensity::Standard,
            ),
            source,
        )
        .expect("projection is valid")
        .expect("ordinary cleanup is eligible");
        assert_eq!(result.plan.output_add_on, OutputAddOn::Bullets);
        assert_eq!(result.plan.mode_source, ModeSource::Manual);
        assert_eq!(result.plan.edit_budget, EditBudget::Standard);
        assert_eq!(result.plan.structure, StructureClass::None);
        assert_eq!(result.projection.text, source);
        assert_eq!(result.projection.structure, ProjectedStructure::Unchanged);
        assert!(result.projection.edits.is_empty());
    }

    #[test]
    fn consented_task_bullet_preference_only_projects_source_backed_items() {
        let mut preference = facts(
            OutputMode::Auto,
            AutoScene::IssueBody,
            CleanupIntensity::Heavy,
        );
        preference.preferred_bullets = true;
        let source = "The three tasks are: update API; fix tests; review docs.";
        let result = resolve_cleanup_projection(preference, source)
            .expect("projection is valid")
            .expect("ordinary cleanup is eligible");
        assert_eq!(
            result.projection.text,
            "The three tasks are:\n- update API\n- fix tests\n- review docs."
        );
        assert_eq!(result.plan.output_add_on, OutputAddOn::Bullets);
        assert_eq!(result.plan.mode_source, ModeSource::Preference);
        assert_eq!(result.plan.structure, StructureClass::SourceBackedInference);

        let ambiguous_source = "I will review the docs and ask whether they are ready.";
        let unchanged = resolve_cleanup_projection(preference, ambiguous_source)
            .expect("projection is valid")
            .expect("ordinary cleanup is eligible");
        assert_eq!(unchanged.projection.text, ambiguous_source);
        assert_eq!(unchanged.plan.mode_source, ModeSource::AutoScene);
        assert_eq!(unchanged.plan.output_add_on, OutputAddOn::AutoIssueBody);

        let mut explicit_mode = preference;
        explicit_mode.output_mode = OutputMode::Email;
        let overridden = resolve_cleanup_projection(explicit_mode, source)
            .expect("projection is valid")
            .expect("ordinary cleanup is eligible");
        assert_eq!(overridden.plan.output_add_on, OutputAddOn::Email);
        assert_eq!(overridden.plan.mode_source, ModeSource::Manual);
    }

    #[test]
    fn email_auto_scene_is_restored_when_preferred_list_has_no_source_proof() {
        let mut email = facts(
            OutputMode::Auto,
            AutoScene::Email,
            CleanupIntensity::Standard,
        );
        email.preferred_bullets = true;
        let source = "We will ship the patch. Separately, we should review the docs.";
        let preferred_result = resolve_cleanup_projection(email, source)
            .expect("projection is valid")
            .expect("ordinary cleanup is eligible");
        let mut baseline_facts = email;
        baseline_facts.preferred_bullets = false;
        let baseline_result = resolve_cleanup_projection(baseline_facts, source)
            .expect("baseline projection is valid")
            .expect("ordinary cleanup is eligible");

        assert_eq!(preferred_result, baseline_result);
        assert_eq!(
            preferred_result.projection.structure,
            ProjectedStructure::Paragraph
        );
        assert_eq!(
            preferred_result.projection.text,
            "We will ship the patch.\n\nSeparately, we should review the docs."
        );
        assert_eq!(preferred_result.plan.output_add_on, OutputAddOn::AutoEmail);
        assert_eq!(preferred_result.plan.mode_source, ModeSource::AutoScene);
    }

    #[test]
    fn structure_safe_overrides_heavy_for_source_backed_projection() {
        let source = "The first item is ready. Separately, the release is ready.";
        let result = resolve_cleanup_projection(
            facts(
                OutputMode::MeetingNotes,
                AutoScene::General,
                CleanupIntensity::Heavy,
            ),
            source,
        )
        .expect("projection is valid")
        .expect("ordinary cleanup is eligible");
        assert_eq!(result.plan.output_add_on, OutputAddOn::MeetingNotes);
        assert_eq!(result.plan.edit_budget, EditBudget::StructureSafe);
        assert_eq!(result.plan.structure, StructureClass::SourceBackedInference);
        assert_eq!(result.projection.structure, ProjectedStructure::Paragraph);
        assert_eq!(
            result.projection.text,
            "The first item is ready.\n\nSeparately, the release is ready."
        );
    }

    #[test]
    fn model_selected_paragraphs_are_live_auto_only_and_keep_the_selected_edit_budget() {
        let mut eligible = facts(
            OutputMode::Auto,
            AutoScene::General,
            CleanupIntensity::Standard,
        );
        eligible.allow_model_selected_paragraphs = true;
        let source = "The release is ready. Separately, the next review starts tomorrow.";
        let result = resolve_cleanup_projection(eligible, source)
            .expect("projection is valid")
            .expect("ordinary cleanup is eligible");
        assert_eq!(result.projection.text, source);
        assert_eq!(
            result.plan.structure,
            StructureClass::ModelSelectedParagraphs
        );
        assert_eq!(result.plan.edit_budget, EditBudget::Standard);

        let mut explicit_mode = eligible;
        explicit_mode.output_mode = OutputMode::Email;
        assert_eq!(
            resolve_cleanup_projection(explicit_mode, source)
                .unwrap()
                .unwrap()
                .plan
                .structure,
            StructureClass::None
        );

        let mut ineligible = eligible;
        ineligible.allow_model_selected_paragraphs = false;
        ineligible.output_mode = OutputMode::MeetingNotes;
        let structure_source = "The first item is ready. Separately, the release is ready.";
        assert_eq!(
            resolve_cleanup_projection(ineligible, structure_source)
                .unwrap()
                .unwrap()
                .plan
                .edit_budget,
            EditBudget::StructureSafe
        );

        let already_structured =
            resolve_cleanup_projection(eligible, "First line.\n\nSecond line.")
                .unwrap()
                .unwrap();
        assert_eq!(
            already_structured.plan.structure,
            StructureClass::SpokenOrRendered
        );
        assert_eq!(
            already_structured.plan.edit_budget,
            EditBudget::StructureSafe
        );
    }

    #[test]
    fn coding_prompt_multiline_can_use_auto_paragraphs_while_code_and_unknown_cannot() {
        let mut codex_prompt = facts(
            OutputMode::Auto,
            AutoScene::DeveloperPrompt,
            CleanupIntensity::Light,
        );
        codex_prompt.focus_kind = FocusKind::CodingPrompt;
        codex_prompt.allow_model_selected_paragraphs = true;
        let source = "Please review the implementation. Also check the tests.";
        assert_eq!(
            resolve_cleanup_projection(codex_prompt, source)
                .unwrap()
                .unwrap()
                .plan
                .structure,
            StructureClass::ModelSelectedParagraphs
        );

        let mut code = codex_prompt;
        code.focus_kind = FocusKind::Code;
        assert_eq!(
            resolve_cleanup_projection(code, source)
                .unwrap()
                .unwrap()
                .plan
                .structure,
            StructureClass::None
        );

        let mut unknown = codex_prompt;
        unknown.focus_kind = FocusKind::Unknown;
        assert_eq!(
            resolve_cleanup_projection(unknown, source)
                .unwrap()
                .unwrap()
                .plan
                .structure,
            StructureClass::None
        );
    }

    #[test]
    fn existing_layout_is_preserved_and_forces_structure_safe() {
        let source = "1. Check the logs.\n2. Restart the service.";
        let result = resolve_cleanup_projection(
            facts(
                OutputMode::Email,
                AutoScene::WorkChat,
                CleanupIntensity::Heavy,
            ),
            source,
        )
        .expect("projection is valid")
        .expect("ordinary cleanup is eligible");
        assert_eq!(result.plan.output_add_on, OutputAddOn::Email);
        assert_eq!(result.plan.edit_budget, EditBudget::StructureSafe);
        assert_eq!(result.plan.structure, StructureClass::SpokenOrRendered);
        assert_eq!(result.projection.text, source);
        assert!(result.projection.edits.is_empty());
    }

    #[test]
    fn bypasses_off_code_translation_mismatch_and_ineligible_routes() {
        let source = "The dictated text.";
        assert!(resolve_cleanup_projection(
            facts(OutputMode::Auto, AutoScene::General, CleanupIntensity::Off),
            source
        )
        .expect("no projection error")
        .is_none());

        assert!(resolve_cleanup_projection(
            facts(
                OutputMode::Code,
                AutoScene::DeveloperPrompt,
                CleanupIntensity::Heavy
            ),
            source
        )
        .expect("no projection error")
        .is_none());

        let mut no_route = facts(
            OutputMode::Auto,
            AutoScene::General,
            CleanupIntensity::Heavy,
        );
        no_route.model_request_eligible = false;
        assert!(resolve_cleanup_projection(no_route, source)
            .expect("no projection error")
            .is_none());

        let mut wrong_operation = facts(
            OutputMode::Auto,
            AutoScene::General,
            CleanupIntensity::Standard,
        );
        wrong_operation.operation = OutputOperation::Other;
        assert!(resolve_cleanup_projection(wrong_operation, source)
            .expect("no projection error")
            .is_none());

        assert!(resolve_cleanup_projection(
            facts(
                OutputMode::Translation,
                AutoScene::General,
                CleanupIntensity::Standard,
            ),
            "First paragraph.\n\nSecond paragraph."
        )
        .expect("no projection error")
        .is_some_and(|result| {
            result.plan.output_add_on == OutputAddOn::Translation
                && result.plan.edit_budget == EditBudget::Standard
                && result.plan.structure == StructureClass::SpokenOrRendered
                && result.requested_layout == OutputLayoutMode::None
        }));
    }

    #[test]
    fn multiline_capability_does_not_follow_coarse_email_or_chat_focus() {
        assert!(!can_project_layout(FocusKind::Email, false));
        assert!(!can_project_layout(FocusKind::Chat, false));
        assert!(!can_project_layout(FocusKind::Form, true));
        assert!(can_project_layout(FocusKind::Email, true));
        assert!(can_project_layout(FocusKind::Chat, true));
    }

    #[test]
    fn auto_scene_uses_existing_focused_profile_without_merging_surfaces() {
        assert_eq!(
            auto_scene_for_context(
                "code.codex",
                ContextFamily::PromptOrCode,
                FocusKind::CodingPrompt,
            ),
            AutoScene::DeveloperPrompt
        );
        assert_eq!(
            auto_scene_for_context(
                "developer.web",
                ContextFamily::Document,
                FocusKind::Document,
            ),
            AutoScene::IssueBody
        );
        assert_eq!(
            auto_scene_for_context(
                "notes.native",
                ContextFamily::NotesJournaling,
                FocusKind::Document,
            ),
            AutoScene::Notes
        );
        assert_eq!(
            auto_scene_for_context("chat.focused", ContextFamily::WorkChat, FocusKind::Chat),
            AutoScene::WorkChat
        );
        assert_eq!(
            auto_scene_for_context(
                "chat.personal",
                ContextFamily::PersonalChat,
                FocusKind::Chat,
            ),
            AutoScene::PersonalChat
        );
    }

    #[test]
    fn actual_email_preparation_keeps_canonical_text_and_builds_a_separate_basis() {
        let settings = crate::store::Settings::default();
        let source = "Hello Alex please review the change.";
        let prepared = crate::prepare_cleanup_transcript_for_scene(
            None,
            &settings,
            source,
            crate::context::ContextFamily::Email,
            0.9,
            FocusKind::Email,
            None,
            false,
        );
        assert!(prepared.scene_email_layout_applied);
        assert!(prepared.text.contains('\n'));
        assert!(!prepared.output_projection_basis.contains('\n'));

        let canonical = prepared.text.clone();
        let projection_source =
            crate::prepare_cleanup_output_projection(&prepared, &settings, true, false, false)
                .expect("eligible scene-only layout has a proven alternate source");
        assert_eq!(prepared.text, canonical);
        assert!(!projection_source.text.contains('\n'));

        let result = resolve_cleanup_projection(
            facts(OutputMode::Email, AutoScene::Email, CleanupIntensity::Heavy),
            &projection_source.text,
        )
        .expect("email projection is valid")
        .expect("ordinary cleanup is eligible");
        assert_eq!(result.plan.output_add_on, OutputAddOn::Email);
        assert_eq!(result.plan.edit_budget, EditBudget::StructureSafe);
        assert_eq!(result.plan.structure, StructureClass::SourceBackedInference);
        assert!(result.projection.text.contains('\n'));
    }
}
