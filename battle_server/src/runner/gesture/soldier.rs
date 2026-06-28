use battle_core::{config::TARGET_FPS, entity::soldier::Soldier, game::weapon::Weapon};
use rand::Rng;

use crate::runner::Runner;

impl Runner {
    pub fn soldier_reloading_end(&self, soldier: &Soldier, weapon: &Weapon) -> u64 {
        // TODO : Depending multiple factor
        let mut rng = rand::thread_rng();
        
        // [스트레스에 따른 장전 지연] 공포에 질려 손을 떨어 재장전 속도가 크게 감소
        let stress_delay = if soldier.under_fire().is_max() { 60 } else { 0 };

        self.battle_state.frame_i() + TARGET_FPS + weapon.reloading_frames() + rng.gen_range(0..50) + stress_delay
    }

    pub fn soldier_aiming_end(&self, soldier: &Soldier, weapon: &Weapon) -> u64 {
        // TODO : Depending multiple factor
        let mut rng = rand::thread_rng();
        
        // [스트레스에 따른 조준 지연] 
        let stress_delay = if soldier.under_fire().is_max() { 40 } else if soldier.under_fire().is_danger() { 20 } else { 0 };

        // [개선] 기습(Ambush) 보너스: 은엄폐(Hide) 상태이거나 포복(Lying) 중인 경우 조준 시간을 대폭 단축하여 선빵 메리트 부여
        let is_ambush = matches!(soldier.behavior(), battle_core::behavior::Behavior::Hide(_)) 
                     || matches!(soldier.body(), battle_core::behavior::Body::Lying);

        let base_aiming_frames = if is_ambush {
            weapon.aiming_frames() / 2 // 무기 조준 지연 시간 절반
        } else {
            weapon.aiming_frames()
        };

        // [인간적 반응 속도(Human Reaction Delay) 적용]
        // 시야에 적이 들어왔을 때 봇들이 컴퓨터처럼 즉각적으로(Sniper-like) 방아쇠를 당기는 기계적 반응을 차단합니다.
        // 비기습(이동 중 등) 상황에서 적을 조우하면 인지 및 지향 사격 전환에 현실적인 추가 딜레이(약 0.75초 ~ 1.5초)를 강제합니다.
        let human_reaction_time = if is_ambush {
            rng.gen_range(10..30) // 기습 대기 중일 때는 조준점을 맞추고 있으므로 아주 짧은 반응 속도 (0.16s ~ 0.5s)
        } else {
            rng.gen_range(45..90) // 이동 중 조우 시 놀라거나 인지하는 긴 당황 시간 부여 (0.75s ~ 1.5s)
        };

        let base_delay = if is_ambush { TARGET_FPS / 2 } else { TARGET_FPS };

        self.battle_state.frame_i() + base_delay + base_aiming_frames + human_reaction_time + rng.gen_range(0..50) + stress_delay
    }

    pub fn soldier_firing_end(&self, soldier: &Soldier, weapon: &Weapon) -> u64 {
        // TODO : Depending multiple factor like weapon, riffle or single shot etc
        let mut rng = rand::thread_rng();
        
        // [스트레스에 따른 사격 후 딜레이 증가] 사격 직후 숨고 다시 몸을 추스르는 데 걸리는 지연 시간
        let stress_delay = if soldier.under_fire().is_max() { 30 } else if soldier.under_fire().is_danger() { 15 } else { 0 };

        // FIXME: firing_frames depend on Shot type
        self.battle_state.frame_i() + 5 + weapon.firing_frames() + rng.gen_range(0..50) + stress_delay
    }
}
