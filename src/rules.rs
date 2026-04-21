use rand::Rng;
use riichienv_core::action::{Action, ActionType};
use riichienv_core::observation::Observation;
use riichienv_core::shanten::calculate_shanten;
use riichienv_core::types::{Meld, MeldType};

const EAST: u8 = 27;
const WHITE: u8 = 31;
const GREEN: u8 = 32;
const RED: u8 = 33;

pub fn choose_action(observation: &Observation) -> Option<Action> {
    let legal_actions = observation.legal_actions_method();
    if legal_actions.is_empty() {
        return None;
    }

    if let Some(action) = legal_actions.iter().find(|action| matches!(action.action_type, ActionType::Tsumo | ActionType::Ron)) {
        return Some(action.clone());
    }

    if let Some(action) = legal_actions.iter().find(|action| action.action_type == ActionType::Riichi) {
        return Some(action.clone());
    }

    let context = RuleContext::new(observation);

    if context.any_opponent_riichi()
        && context.current_shanten >= 2
        && let Some(action) = choose_betaori_discard(&context, &legal_actions)
    {
        return Some(action);
    }

    if let Some(action) = choose_kan_action(&context, &legal_actions) {
        return Some(action);
    }

    if let Some(action) = choose_yakuhai_pon_action(&context, &legal_actions) {
        return Some(action);
    }

    if context.has_secured_yakuhai() {
        if let Some(action) = choose_improving_open_meld_action(&context, &legal_actions) {
            return Some(action);
        }

        if let Some(action) = choose_daiminkan_action(&context, &legal_actions) {
            return Some(action);
        }
    }

    choose_best_discard(&context, &legal_actions).or_else(|| legal_actions.iter().find(|action| action.action_type == ActionType::Pass).cloned()).or_else(|| legal_actions.first().cloned())
}

fn choose_betaori_discard(context: &RuleContext, legal_actions: &[Action]) -> Option<Action> {
    let candidates: Vec<_> = discard_evaluations(context, legal_actions).into_iter().filter(|evaluation| evaluation.safe_against_riichi_count > 0).collect();
    choose_best_discard_eval(candidates, true).map(|evaluation| evaluation.action)
}

fn choose_kan_action(context: &RuleContext, legal_actions: &[Action]) -> Option<Action> {
    let candidates: Vec<_> = legal_actions
        .iter()
        .filter(|action| matches!(action.action_type, ActionType::Ankan | ActionType::Kakan))
        .filter_map(|action| {
            let new_hand = hand_after_call_action(context.self_hand(), action);
            let new_shanten = calculate_shanten(&new_hand);
            (new_shanten <= context.current_shanten).then_some((action.clone(), new_shanten))
        })
        .collect();
    choose_random(candidates.into_iter().map(|(action, _)| action).collect())
}

fn choose_yakuhai_pon_action(context: &RuleContext, legal_actions: &[Action]) -> Option<Action> {
    let candidates: Vec<_> = legal_actions
        .iter()
        .filter(|action| action.action_type == ActionType::Pon)
        .filter(|action| action.tile.is_some_and(|tile| context.is_yakuhai_tile(tile / 4)))
        .filter_map(|action| {
            let new_hand = hand_after_call_action(context.self_hand(), action);
            let new_shanten = calculate_shanten(&new_hand);
            (new_shanten < context.current_shanten).then(|| CallEvaluation {
                action: action.clone(),
                shanten: new_shanten,
                ukeire: calculate_ukeire(&new_hand, &context.visible_tiles),
            })
        })
        .collect();
    choose_best_call_eval(candidates).map(|evaluation| evaluation.action)
}

fn choose_improving_open_meld_action(context: &RuleContext, legal_actions: &[Action]) -> Option<Action> {
    let candidates: Vec<_> = legal_actions
        .iter()
        .filter(|action| matches!(action.action_type, ActionType::Chi | ActionType::Pon))
        .filter_map(|action| {
            let new_hand = hand_after_call_action(context.self_hand(), action);
            let new_shanten = calculate_shanten(&new_hand);
            (new_shanten < context.current_shanten).then(|| CallEvaluation {
                action: action.clone(),
                shanten: new_shanten,
                ukeire: calculate_ukeire(&new_hand, &context.visible_tiles),
            })
        })
        .collect();
    choose_best_call_eval(candidates).map(|evaluation| evaluation.action)
}

fn choose_daiminkan_action(context: &RuleContext, legal_actions: &[Action]) -> Option<Action> {
    let candidates: Vec<_> = legal_actions
        .iter()
        .filter(|action| action.action_type == ActionType::Daiminkan)
        .filter_map(|action| {
            let new_hand = hand_after_call_action(context.self_hand(), action);
            let new_shanten = calculate_shanten(&new_hand);
            (new_shanten <= context.current_shanten).then_some(action.clone())
        })
        .collect();
    choose_random(candidates)
}

fn choose_best_discard(context: &RuleContext, legal_actions: &[Action]) -> Option<Action> {
    choose_best_discard_eval(discard_evaluations(context, legal_actions), false).map(|evaluation| evaluation.action)
}

fn discard_evaluations(context: &RuleContext, legal_actions: &[Action]) -> Vec<DiscardEvaluation> {
    legal_actions
        .iter()
        .filter(|action| action.action_type == ActionType::Discard)
        .filter_map(|action| {
            let tile = action.tile?;
            let mut new_hand = context.self_hand().to_vec();
            remove_one_tile(&mut new_hand, tile.into())?;
            Some(DiscardEvaluation {
                action: action.clone(),
                shanten: calculate_shanten(&new_hand),
                ukeire: calculate_ukeire(&new_hand, &context.visible_tiles),
                safe_against_riichi_count: context.safe_against_riichi_count((tile / 4).into()),
            })
        })
        .collect()
}

fn choose_best_discard_eval(evaluations: Vec<DiscardEvaluation>, prioritize_safety: bool) -> Option<DiscardEvaluation> {
    let best = evaluations.into_iter().fold(None, |best, evaluation| match best {
        None => Some(evaluation),
        Some(current) => match compare_discard_eval(&evaluation, &current, prioritize_safety) {
            std::cmp::Ordering::Less => Some(evaluation),
            std::cmp::Ordering::Equal => {
                let chosen = choose_random(vec![current, evaluation])?;
                Some(chosen)
            }
            std::cmp::Ordering::Greater => Some(current),
        },
    });
    best
}

fn compare_discard_eval(lhs: &DiscardEvaluation, rhs: &DiscardEvaluation, prioritize_safety: bool) -> std::cmp::Ordering {
    if prioritize_safety {
        rhs.safe_against_riichi_count.cmp(&lhs.safe_against_riichi_count).then_with(|| lhs.shanten.cmp(&rhs.shanten)).then_with(|| rhs.ukeire.cmp(&lhs.ukeire))
    } else {
        lhs.shanten.cmp(&rhs.shanten).then_with(|| rhs.ukeire.cmp(&lhs.ukeire)).then_with(|| rhs.safe_against_riichi_count.cmp(&lhs.safe_against_riichi_count))
    }
}

fn choose_best_call_eval(evaluations: Vec<CallEvaluation>) -> Option<CallEvaluation> {
    let best = evaluations.into_iter().fold(None, |best, evaluation| match best {
        None => Some(evaluation),
        Some(current) => {
            let ordering = evaluation.shanten.cmp(&current.shanten).then_with(|| current.ukeire.cmp(&evaluation.ukeire));
            match ordering {
                std::cmp::Ordering::Less => Some(evaluation),
                std::cmp::Ordering::Equal => choose_random(vec![current, evaluation]),
                std::cmp::Ordering::Greater => Some(current),
            }
        }
    });
    best
}

fn hand_after_call_action(hand: &[u32], action: &Action) -> Vec<u32> {
    let mut new_hand = hand.to_vec();
    match action.action_type {
        ActionType::Ankan => {
            for &tile in &action.consume_tiles {
                let _ = remove_one_tile(&mut new_hand, tile as u32);
            }
        }
        ActionType::Kakan => {
            if let Some(tile) = action.tile {
                let _ = remove_one_tile(&mut new_hand, tile as u32);
            }
        }
        ActionType::Chi | ActionType::Pon | ActionType::Daiminkan => {
            for &tile in &action.consume_tiles {
                let _ = remove_one_tile(&mut new_hand, tile as u32);
            }
        }
        _ => {}
    }
    new_hand
}

fn calculate_ukeire(hand: &[u32], visible_tiles: &[u32]) -> u32 {
    let current_shanten = calculate_shanten(hand);
    let mut visible_counts = [0u8; 34];
    let mut hand_counts = [0u8; 34];

    for &tile in visible_tiles {
        let tile_type = (tile / 4) as usize;
        if tile_type < 34 {
            visible_counts[tile_type] = visible_counts[tile_type].saturating_add(1);
        }
    }

    for &tile in hand {
        let tile_type = (tile / 4) as usize;
        if tile_type < 34 {
            hand_counts[tile_type] = hand_counts[tile_type].saturating_add(1);
        }
    }

    let mut ukeire = 0;
    for tile_type in 0..34u32 {
        if hand_counts[tile_type as usize] >= 4 {
            continue;
        }

        let mut test_hand = hand.to_vec();
        test_hand.push(tile_type * 4);
        if calculate_shanten(&test_hand) < current_shanten {
            let seen = visible_counts[tile_type as usize] as u32 + hand_counts[tile_type as usize] as u32;
            ukeire += 4u32.saturating_sub(seen);
        }
    }
    ukeire
}

fn remove_one_tile(hand: &mut Vec<u32>, tile: u32) -> Option<()> {
    let target_type = tile / 4;
    let position = hand.iter().position(|candidate| *candidate / 4 == target_type)?;
    hand.remove(position);
    Some(())
}

fn choose_random<T>(items: Vec<T>) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let mut rng = rand::rng();
    let index = rng.random_range(0..items.len());
    items.into_iter().nth(index)
}

struct RuleContext {
    player_id: usize,
    self_hand: Vec<u32>,
    self_melds: Vec<Meld>,
    opponent_discards: [Vec<u32>; 4],
    opponent_riichi: [bool; 4],
    visible_tiles: Vec<u32>,
    current_shanten: i32,
    yakuhai_tile_types: Vec<u8>,
}

impl RuleContext {
    fn new(observation: &Observation) -> Self {
        let player_id = observation.player_id as usize;
        let self_hand = observation.hands[player_id].clone();
        let self_melds = observation.melds[player_id].clone();
        let yakuhai_tile_types = yakuhai_tile_types(observation);
        let visible_tiles = collect_visible_tiles(observation, player_id);

        Self {
            player_id,
            self_hand: self_hand.clone(),
            self_melds,
            opponent_discards: observation.discards.clone(),
            opponent_riichi: observation.riichi_declared,
            visible_tiles,
            current_shanten: calculate_shanten(&self_hand),
            yakuhai_tile_types,
        }
    }

    fn self_hand(&self) -> &[u32] {
        &self.self_hand
    }

    fn any_opponent_riichi(&self) -> bool {
        self.opponent_riichi.iter().enumerate().any(|(player, declared)| player != self.player_id && *declared)
    }

    fn safe_against_riichi_count(&self, tile_type: u32) -> u8 {
        self.opponent_riichi.iter().enumerate().filter(|(player, declared)| *player != self.player_id && **declared).filter(|(player, _)| self.opponent_discards[*player].iter().any(|discard| discard / 4 == tile_type)).count() as u8
    }

    fn has_secured_yakuhai(&self) -> bool {
        self.self_melds.iter().any(|meld| meld.tiles.first().is_some_and(|tile| self.is_yakuhai_tile(tile / 4)) && matches!(meld.meld_type, MeldType::Pon | MeldType::Daiminkan | MeldType::Ankan | MeldType::Kakan)) || self.yakuhai_tile_types.iter().any(|&tile_type| count_tile_type(&self.self_hand, tile_type as u32) >= 3)
    }

    fn is_yakuhai_tile(&self, tile_type: u8) -> bool {
        self.yakuhai_tile_types.contains(&tile_type)
    }
}

fn yakuhai_tile_types(observation: &Observation) -> Vec<u8> {
    let seat_wind = EAST + ((observation.player_id + 4 - observation.oya) % 4);
    let mut tile_types = vec![WHITE, GREEN, RED, EAST + observation.round_wind, seat_wind];
    tile_types.sort_unstable();
    tile_types.dedup();
    tile_types
}

fn collect_visible_tiles(observation: &Observation, player_id: usize) -> Vec<u32> {
    let mut visible_tiles = Vec::new();
    visible_tiles.extend(observation.dora_indicators.iter().copied());

    for (idx, discards) in observation.discards.iter().enumerate() {
        if idx == player_id {
            continue;
        }
        visible_tiles.extend(discards.iter().copied());
    }

    for melds in &observation.melds {
        for meld in melds {
            visible_tiles.extend(meld.tiles.iter().map(|&tile| tile as u32));
        }
    }

    visible_tiles
}

fn count_tile_type(hand: &[u32], tile_type: u32) -> usize {
    hand.iter().filter(|tile| **tile / 4 == tile_type).count()
}

#[derive(Debug)]
struct DiscardEvaluation {
    action: Action,
    shanten: i32,
    ukeire: u32,
    safe_against_riichi_count: u8,
}

#[derive(Debug)]
struct CallEvaluation {
    action: Action,
    shanten: i32,
    ukeire: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use riichienv_core::action::ActionType;

    #[test]
    fn remove_one_tile_matches_tile_type() {
        let mut hand = vec![0, 1, 8];
        remove_one_tile(&mut hand, 2);
        assert_eq!(hand, vec![1, 8]);
    }

    #[test]
    fn hand_after_kakan_removes_added_tile_only() {
        let hand = vec![0, 1, 2, 3, 16];
        let action = Action::new(ActionType::Kakan, Some(16), vec![0, 1, 2], Some(0));
        assert_eq!(hand_after_call_action(&hand, &action), vec![0, 1, 2, 3]);
    }
}
