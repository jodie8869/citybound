use kay::{World, MachineID,   ActorSystem};
use compact::{CHashMap, COption};
use descartes::{P2, AreaError, LinePath};
use super::{Plan, PlanHistory, PlanResult,  GestureID, ProjectID,
PlanManager, PlanManagerID, Gesture, GestureIntent,
KnownHistoryState, KnownProjectState, ProjectUpdate,
KnownPlanResultState,
ActionGroups};
use super::ui::PlanningUIID;
use log::error;
const LOG_T: &str = "Planning Interaction";

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct ControlPointRef(pub GestureID, pub usize);

#[derive(Compact, Clone)]
pub struct PlanManagerUIState {
    pub current_project: ProjectID,
    gesture_ongoing: bool,
    current_preview: COption<PlanHistory>,
    current_result_preview: COption<PlanResult>,
    current_action_preview: COption<ActionGroups>,
}

impl PlanManager {
    pub fn get_all_plans(
        &mut self,
        ui: PlanningUIID,
        known_master: &KnownHistoryState,
        known_projects: &CHashMap<ProjectID, KnownProjectState>,
        world: &mut World,
    ) {
        let master_update = self.master_plan.update_for(known_master);
        let mut unmatched_known_projects = known_projects
            .keys()
            .cloned()
            .collect::<::std::collections::HashSet<_>>();
        let project_updates = self
            .projects
            .pairs()
            .map(|(project_id, project)| {
                (
                    *project_id,
                    known_projects
                        .get(*project_id)
                        .map(|known_state| {
                            unmatched_known_projects.remove(project_id);
                            project.update_for(known_state)
                        })
                        .unwrap_or_else(|| ProjectUpdate::ChangedCompletely(project.clone())),
                )
            })
            .collect::<Vec<_>>();
        let project_updates_with_removals = project_updates
            .into_iter()
            .chain(
                unmatched_known_projects
                    .into_iter()
                    .map(|unmatched_id| (unmatched_id, ProjectUpdate::Removed)),
            )
            .collect();
        ui.on_plans_update(master_update, project_updates_with_removals, world);
    }

    pub fn get_project_preview_update(
        &mut self,
        ui: PlanningUIID,
        project_id: ProjectID,
        known_result: &KnownPlanResultState,
        world: &mut World,
    ) {
        // TODO: this is a super ugly hack until we get rid of the native UI
        let needs_switch = if let Some(ui_state) = self.ui_state.get_mut(world.local_machine_id()) {
            ui_state.current_project != project_id
        } else {
            false
        };

        if needs_switch {
            self.switch_to(MachineID(0), project_id, world);
        }

        let (plan_history, maybe_result, maybe_actions) =
            self.try_ensure_preview(world.local_machine_id(), project_id, world);

        if let (Some(result), Some(actions)) = (maybe_result, maybe_actions) {
            ui.on_project_preview_update(
                project_id,
                plan_history.clone(),
                result.update_for(known_result),
                actions.clone(),
                world,
            );
        }
    }
}

impl PlanManager {
    pub fn switch_to(&mut self, machine: MachineID, project_id: ProjectID, _: &mut World) {
        self.ui_state.insert(
            machine,
            PlanManagerUIState {
                current_project: project_id,
                gesture_ongoing: false,
                current_preview: COption(None),
                current_result_preview: COption(None),
                current_action_preview: COption(None),
            },
        );
    }

    pub fn clear_previews(&mut self, project_id: ProjectID) {
        for state in self
            .ui_state
            .values_mut()
            .filter(|state| state.current_project == project_id)
        {
            state.current_preview = COption(None);
            state.current_result_preview = COption(None);
            state.current_action_preview = COption(None);
        }
    }

    #[allow(mutable_transmutes)]
    pub fn try_ensure_preview(
        &self,
        machine_id: MachineID,
        project_id: ProjectID,
        log_in: &mut World,
    ) -> (&PlanHistory, Option<&PlanResult>, &Option<ActionGroups>) {
        let ui_state = self
            .ui_state
            .get(machine_id)
            .expect("Should already have a ui state for this machine");

        // super ugly of course, maybe we can use a cell or similar in the future
        unsafe {
            let ui_state_mut: &mut PlanManagerUIState =
                &mut *(ui_state as *const PlanManagerUIState as *mut PlanManagerUIState);
            if ui_state.current_project != project_id {
                ui_state_mut.current_preview = COption(None);
            }

            if ui_state.current_preview.is_none() {
                let preview_plan = self
                    .projects
                    .get(project_id)
                    .unwrap()
                    .apply_to_with_ongoing(&self.master_plan);

                match preview_plan.calculate_result() {
                    Ok(preview_plan_result) => {
                        let (actions, _) = self.master_result.actions_to(&preview_plan_result);
                        ui_state_mut.current_result_preview = COption(Some(preview_plan_result));
                        ui_state_mut.current_action_preview = COption(Some(actions));
                    }
                    Err(err) => match err {
                        AreaError::LeftOver(string) => {
                            error(
                                LOG_T,
                                format!("Preview Plan Error: {}", string),
                                self.id,
                                log_in,
                            );
                        }
                        _ => {
                            error(
                                LOG_T,
                                format!("Preview Plan Error: {:?}", err),
                                self.id,
                                log_in,
                            );
                        }
                    },
                }

                ui_state_mut.current_preview = COption(Some(preview_plan));
            }
        }

        (
            ui_state.current_preview.as_ref().unwrap(),
            ui_state.current_result_preview.as_ref(),
            &*ui_state.current_action_preview,
        )
    }

    pub fn start_new_gesture(
        &mut self,
        project_id: ProjectID,
        machine_id: MachineID,
        new_gesture_id: GestureID,
        intent: &GestureIntent,
        start: P2,
        _: &mut World,
    ) {
        let new_gesture = Gesture::new(vec![start].into(), intent.clone());

        let new_step = Plan::from_gestures(Some((new_gesture_id, new_gesture)));

        self.projects
            .get_mut(project_id)
            .unwrap()
            .set_ongoing_step(new_step);
        self.projects.get_mut(project_id).unwrap().start_new_step();

        self.ui_state
            .get_mut(machine_id)
            .expect("should already have ui state")
            .gesture_ongoing = true;

        self.clear_previews(project_id);
    }

    pub fn finish_gesture(&mut self, machine_id: MachineID, _: &mut World) {
        self.ui_state
            .get_mut(machine_id)
            .expect("should already have ui state")
            .gesture_ongoing = false;
    }

    pub fn add_control_point(
        &mut self,
        project_id: ProjectID,
        gesture_id: GestureID,
        new_point: P2,
        add_to_end: bool,
        commit: bool,
        _: &mut World,
    ) {
        let new_step = {
            let current_gesture = self.get_current_version_of(gesture_id, project_id);

            let changed_gesture = if add_to_end {
                Gesture {
                    points: current_gesture
                        .points
                        .iter()
                        .cloned()
                        .chain(Some(new_point))
                        .collect(),
                    ..current_gesture.clone()
                } //.simplify()
            } else {
                Gesture {
                    points: Some(new_point)
                        .into_iter()
                        .chain(current_gesture.points.iter().cloned())
                        .collect(),
                    ..current_gesture.clone()
                } //.simplify()
            };

            Plan::from_gestures(Some((gesture_id, changed_gesture)))
        };

        self.projects
            .get_mut(project_id)
            .unwrap()
            .set_ongoing_step(new_step);

        if commit {
            self.projects.get_mut(project_id).unwrap().start_new_step();
        }

        self.clear_previews(project_id);
    }

    pub fn insert_control_point(
        &mut self,
        project_id: ProjectID,
        gesture_id: GestureID,
        new_point: P2,
        commit: bool,
        _: &mut World,
    ) {
        let new_step = {
            let current_gesture = self.get_current_version_of(gesture_id, project_id);

            let new_point_idx = LinePath::new(current_gesture.points.clone())
                .and_then(|path| {
                    path.project(new_point)
                        .and_then(|(inserted_along, _projected)| {
                            path.distances
                                .iter()
                                .position(|point_i_along| *point_i_along >= inserted_along)
                        })
                })
                .unwrap_or(current_gesture.points.len());

            let changed_gesture = Gesture {
                points: current_gesture.points[..new_point_idx]
                    .iter()
                    .cloned()
                    .chain(Some(new_point))
                    .chain(current_gesture.points[new_point_idx..].iter().cloned())
                    .collect(),
                ..current_gesture.clone()
            }; //.simplify()

            Plan::from_gestures(Some((gesture_id, changed_gesture)))
        };

        self.projects
            .get_mut(project_id)
            .unwrap()
            .set_ongoing_step(new_step);

        if commit {
            self.projects.get_mut(project_id).unwrap().start_new_step();
        }

        self.clear_previews(project_id);
    }

    pub fn move_control_point(
        &mut self,
        project_id: ProjectID,
        gesture_id: GestureID,
        point_index: u32,
        new_position: P2,
        is_move_finished: bool,
        _: &mut World,
    ) {
        let current_change = {
            let current_gesture = self.get_current_version_of(gesture_id, project_id);

            if point_index as usize >= current_gesture.points.len() {
                return;
            }

            let mut new_gesture_points = current_gesture.points.clone();
            new_gesture_points[point_index as usize] = new_position;

            let new_gesture = Gesture {
                points: new_gesture_points,
                ..current_gesture.clone()
            };

            Plan::from_gestures(Some((gesture_id, new_gesture)))
        };

        self.projects
            .get_mut(project_id)
            .unwrap()
            .set_ongoing_step(current_change);

        // TODO: can we update only part of the preview
        // for better rendering performance while dragging?
        self.clear_previews(project_id);

        if is_move_finished {
            self.projects.get_mut(project_id).unwrap().start_new_step();
        }
    }

    pub fn split_gesture(
        &mut self,
        project_id: ProjectID,
        gesture_id: GestureID,
        split_at: P2,
        commit: bool,
        _: &mut World,
    ) {
        let maybe_new_step = {
            let current_gesture = self.get_current_version_of(gesture_id, project_id);

            if let Some((split_at_idx, point_before, point_after)) =
                LinePath::new(current_gesture.points.clone()).and_then(|path| {
                    path.project(split_at)
                        .and_then(|(split_along, _projected)| {
                            path.distances
                                .iter()
                                .position(|point_i_along| *point_i_along >= split_along)
                                .map(|idx| {
                                    let point_before = path.along(split_along - 5.0);
                                    let point_after = path.along(split_along + 5.0);
                                    (idx, point_before, point_after)
                                })
                        })
                })
            {
                let first_half = Gesture {
                    points: current_gesture.points[..split_at_idx]
                        .iter()
                        .cloned()
                        .chain(Some(point_before))
                        .collect(),
                    ..current_gesture.clone()
                }; //.simplify()

                let second_half = Gesture {
                    points: Some(point_after)
                        .into_iter()
                        .chain(current_gesture.points[split_at_idx..].iter().cloned())
                        .collect(),
                    ..current_gesture.clone()
                }; //.simplify()

                Some(Plan::from_gestures(vec![
                    (gesture_id, first_half),
                    (GestureID::new(), second_half),
                ]))
            } else {
                None
            }
        };

        if let Some(new_step) = maybe_new_step {
            self.projects
                .get_mut(project_id)
                .unwrap()
                .set_ongoing_step(new_step);

            if commit {
                self.projects.get_mut(project_id).unwrap().start_new_step();
            }

            self.clear_previews(project_id);
        }
    }

    pub fn set_intent(
        &mut self,
        project_id: ProjectID,
        gesture_id: GestureID,
        new_intent: &GestureIntent,
        is_move_finished: bool,
        _: &mut World,
    ) {
        let current_change = {
            let current_gesture = self.get_current_version_of(gesture_id, project_id);

            let new_gesture = Gesture {
                intent: new_intent.clone(),
                ..current_gesture.clone()
            };

            Plan::from_gestures(Some((gesture_id, new_gesture)))
        };

        self.projects
            .get_mut(project_id)
            .unwrap()
            .set_ongoing_step(current_change);

        // TODO: can we update only part of the preview
        // for better rendering performance while dragging?
        self.clear_previews(project_id);

        if is_move_finished {
            self.projects.get_mut(project_id).unwrap().start_new_step();
        }
    }

    pub fn undo(&mut self, project_id: ProjectID, _: &mut World) {
        self.projects.get_mut(project_id).unwrap().undo();
        self.clear_previews(project_id);
    }

    pub fn redo(&mut self, project_id: ProjectID, _: &mut World) {
        self.projects.get_mut(project_id).unwrap().redo();
        self.clear_previews(project_id);
    }
}

pub fn setup(system: &mut ActorSystem) {
    auto_setup(system);
}

pub mod kay_auto;
pub use self::kay_auto::*;
