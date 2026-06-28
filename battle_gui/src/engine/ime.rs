use ggez::winit::event::VirtualKeyCode;

// 한글 유니코드 공식 상수
const HANGUL_BASE: u32 = 0xAC00;

const JUNG_COUNT: u32 = 21;
const JONG_COUNT: u32 = 28;

// 초성 19자
const CHO_MAP: [char; 19] = [
    'ㄱ', 'ㄲ', 'ㄴ', 'ㄷ', 'ㄸ', 'ㄹ', 'ㅁ', 'ㅂ', 'ㅃ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅉ', 'ㅊ', 'ㅋ', 'ㅌ', 'ㅍ', 'ㅎ',
];

// 중성 21자
const JUNG_MAP: [char; 21] = [
    'ㅏ', 'ㅐ', 'ㅑ', 'ㅒ', 'ㅓ', 'ㅔ', 'ㅕ', 'ㅖ', 'ㅗ', 'ㅘ', 'ㅙ', 'ㅚ', 'ㅛ', 'ㅜ', 'ㅝ', 'ㅞ', 'ㅟ', 'ㅠ', 'ㅡ', 'ㅢ', 'ㅣ',
];

/// 인게임 자체 한글 조합 상태 머신 (Hangul Automata - 2벌식)
pub struct HangulAutomata {
    pub is_korean_mode: bool,
    cho: Option<u32>,
    jung: Option<u32>,
    jong: Option<u32>,
}

impl HangulAutomata {
    pub fn new() -> Self {
        Self {
            is_korean_mode: false,
            cho: None,
            jung: None,
            jong: None,
        }
    }

    pub fn toggle_mode(&mut self) {
        self.is_korean_mode = !self.is_korean_mode;
        self.clear();
    }

    pub fn is_composing(&self) -> bool {
        self.cho.is_some() || self.jung.is_some() || self.jong.is_some()
    }

    pub fn clear(&mut self) {
        self.cho = None;
        self.jung = None;
        self.jong = None;
    }

    /// 현재 조합 중인 상태를 지우고 버퍼에서 한 글자를 뺍니다.
    pub fn handle_backspace(&mut self, buffer: &mut String) -> bool {
        if !self.is_composing() {
            return false; // 조합 중인 글자가 없으면 Egui 기본 백스페이스로 넘김
        }

        // 종성이 있으면 종성만 제거
        if self.jong.is_some() && self.jong != Some(0) {
            self.jong = None;
        } 
        // 중성이 있으면 중성만 제거
        else if self.jung.is_some() {
            self.jung = None;
        } 
        // 초성만 있으면 초성 제거하고 조합 상태 종료
        else if self.cho.is_some() {
            self.cho = None;
        }

        buffer.pop(); // 기존 글자 지우기
        if self.is_composing() {
            buffer.push(self.get_composed_char()); // 분해된 글자 다시 그리기
        }
        true
    }

    /// 현재 초/중/종성 인덱스를 바탕으로 하나의 유니코드 한글 문자를 생성합니다.
    pub fn get_composed_char(&self) -> char {
        if let Some(cho) = self.cho {
            if let Some(jung) = self.jung {
                let jong = self.jong.unwrap_or(0);
                let unicode_val = HANGUL_BASE + (cho * JUNG_COUNT * JONG_COUNT) + (jung * JONG_COUNT) + jong;
                std::char::from_u32(unicode_val).unwrap_or(' ')
            } else {
                CHO_MAP[cho as usize]
            }
        } else if let Some(jung) = self.jung {
            JUNG_MAP[jung as usize]
        } else {
            ' '
        }
    }

    /// QWERTY 입력을 받아 오토마타 상태 머신을 돌리고 버퍼 문자열을 갱신합니다.
    pub fn process_key(&mut self, keycode: VirtualKeyCode, is_shift: bool, buffer: &mut String) -> bool {
        let mapped = match Self::map_qwerty_to_hangul(keycode, is_shift) {
            Some(v) => v,
            None => {
                // 한글 맵핑이 안 되는 키(숫자, 기호 등)가 들어오면 현재 조합을 확정(Commit)
                self.clear();
                return false;
            }
        };

        let is_vowel = mapped.0;
        let index = mapped.1;

        // 이미 조합 중인 글자가 화면에 떠 있으면 지우고 다시 렌더링하기 위해 버퍼를 조작
        if self.is_composing() {
            buffer.pop();
        }

        if is_vowel {
            // 모음 입력 처리
            if self.cho.is_some() && self.jung.is_none() {
                // 초성만 있을 때 -> 초성 + 중성 결합
                self.jung = Some(index);
            } else if self.cho.is_some() && self.jung.is_some() && self.jong.is_none() {
                // 겹모음 처리 (예: ㅗ + ㅏ = ㅘ)
                if let Some(combined) = Self::combine_vowels(self.jung.unwrap(), index) {
                    self.jung = Some(combined);
                } else {
                    // 겹모음이 불가능하면 현재 글자 확정 후 새 글자의 중성으로 시작 (예외 케이스)
                    buffer.push(self.get_composed_char());
                    self.clear();
                    self.jung = Some(index);
                }
            } else if self.cho.is_some() && self.jung.is_some() && self.jong.is_some() {
                // 종성이 있는데 모음이 들어오면 -> 겹받침 등 종성을 쪼개서 다음 글자의 초성으로 밀어냄 (전환, 닭알 등)
                let prev_jong = self.jong.unwrap();
                let (remain_jong, new_cho) = Self::split_complex_jong(prev_jong);
                
                self.jong = remain_jong;
                buffer.push(self.get_composed_char()); // 이전 글자 확정
                
                self.clear();
                self.cho = Some(new_cho); // 떼어낸 종성을 새 글자의 초성으로
                self.jung = Some(index);
            } else if self.cho.is_none() && self.jung.is_some() {
                // 빈 상태에서 모음만 있는데 또 모음이 들어오는 경우 (모음만으로 겹모음 형성)
                if let Some(combined) = Self::combine_vowels(self.jung.unwrap(), index) {
                    self.jung = Some(combined);
                } else {
                    buffer.push(self.get_composed_char());
                    self.clear();
                    self.jung = Some(index);
                }
            } else {
                // 완전 빈 상태에서 모음만 입력
                self.clear();
                self.jung = Some(index);
            }
        } else {
            // 자음 입력 처리
            if self.cho.is_none() && self.jung.is_none() {
                // 첫 입력이 자음 -> 초성에 넣음
                self.cho = Some(index);
            } else if self.cho.is_some() && self.jung.is_none() {
                // 초성만 있는데 또 자음이 옴 -> 이전 초성 확정 후 새 초성 시작
                buffer.push(self.get_composed_char());
                self.clear();
                self.cho = Some(index);
            } else if self.cho.is_some() && self.jung.is_some() && self.jong.is_none() {
                // 초+중성 상태에서 자음이 옴 -> 종성으로 변환하여 넣음
                if let Some(jong_idx) = Self::cho_to_jong(index) {
                    self.jong = Some(jong_idx);
                } else {
                    // ㄸ, ㅃ, ㅉ 등 종성에 올 수 없는 자음이면 현재 글자 확정 후 새 글자 초성으로
                    buffer.push(self.get_composed_char());
                    self.clear();
                    self.cho = Some(index);
                }
            } else if self.cho.is_some() && self.jung.is_some() && self.jong.is_some() {
                // 겹받침 처리 (예: ㄱ + ㅅ = ㄳ)
                let current_jong = self.jong.unwrap();
                if let Some(jong_idx) = Self::cho_to_jong(index) {
                    if let Some(combined) = Self::combine_consonants(current_jong, jong_idx) {
                        self.jong = Some(combined);
                    } else {
                        // 겹받침 불가면 현재 글자 확정 후 새 초성
                        buffer.push(self.get_composed_char());
                        self.clear();
                        self.cho = Some(index);
                    }
                } else {
                    buffer.push(self.get_composed_char());
                    self.clear();
                    self.cho = Some(index);
                }
            }
        }

        // 조합된 글자를 다시 Egui 버퍼 맨 끝에 그려줌 (Preedit 시각화)
        if self.is_composing() {
            buffer.push(self.get_composed_char());
        }

        true
    }

    /// QWERTY 자판을 초성/중성 인덱스로 맵핑합니다 (is_vowel, index)
    fn map_qwerty_to_hangul(keycode: VirtualKeyCode, is_shift: bool) -> Option<(bool, u32)> {
        match keycode {
            VirtualKeyCode::R => Some((false, if is_shift { 1 } else { 0 })), // ㄱ/ㄲ
            VirtualKeyCode::S => Some((false, 2)), // ㄴ
            VirtualKeyCode::E => Some((false, if is_shift { 4 } else { 3 })), // ㄷ/ㄸ
            VirtualKeyCode::F => Some((false, 5)), // ㄹ
            VirtualKeyCode::A => Some((false, 6)), // ㅁ
            VirtualKeyCode::Q => Some((false, if is_shift { 8 } else { 7 })), // ㅂ/ㅃ
            VirtualKeyCode::T => Some((false, if is_shift { 10 } else { 9 })), // ㅅ/ㅆ
            VirtualKeyCode::D => Some((false, 11)), // ㅇ
            VirtualKeyCode::W => Some((false, if is_shift { 13 } else { 12 })), // ㅈ/ㅉ
            VirtualKeyCode::C => Some((false, 14)), // ㅊ
            VirtualKeyCode::Z => Some((false, 15)), // ㅋ
            VirtualKeyCode::X => Some((false, 16)), // ㅌ
            VirtualKeyCode::V => Some((false, 17)), // ㅍ
            VirtualKeyCode::G => Some((false, 18)), // ㅎ

            VirtualKeyCode::K => Some((true, 0)), // ㅏ
            VirtualKeyCode::O => Some((true, if is_shift { 3 } else { 1 })), // ㅐ/ㅒ
            VirtualKeyCode::I => Some((true, 2)), // ㅑ
            VirtualKeyCode::J => Some((true, 4)), // ㅓ
            VirtualKeyCode::P => Some((true, if is_shift { 7 } else { 5 })), // ㅔ/ㅖ
            VirtualKeyCode::U => Some((true, 6)), // ㅕ
            VirtualKeyCode::H => Some((true, 8)), // ㅗ
            VirtualKeyCode::Y => Some((true, 12)), // ㅛ
            VirtualKeyCode::N => Some((true, 13)), // ㅜ
            VirtualKeyCode::B => Some((true, 17)), // ㅠ
            VirtualKeyCode::M => Some((true, 18)), // ㅡ
            VirtualKeyCode::L => Some((true, 20)), // ㅣ
            _ => None,
        }
    }

    /// 모음 결합 처리 (ㅗ+ㅏ = ㅘ)
    fn combine_vowels(v1: u32, v2: u32) -> Option<u32> {
        match (v1, v2) {
            (8, 0) => Some(9),   // ㅗ + ㅏ = ㅘ
            (8, 1) => Some(10),  // ㅗ + ㅐ = ㅙ
            (8, 20) => Some(11), // ㅗ + ㅣ = ㅚ
            (13, 4) => Some(14), // ㅜ + ㅓ = ㅝ
            (13, 5) => Some(15), // ㅜ + ㅔ = ㅞ
            (13, 20) => Some(16),// ㅜ + ㅣ = ㅟ
            (18, 20) => Some(19),// ㅡ + ㅣ = ㅢ
            _ => None,
        }
    }

    /// 자음 결합 처리 (겹받침 ㄱ+ㅅ = ㄳ)
    fn combine_consonants(c1: u32, c2: u32) -> Option<u32> {
        match (c1, c2) {
            (1, 19) => Some(3),  // ㄱ + ㅅ = ㄳ
            (4, 22) => Some(5),  // ㄴ + ㅈ = ㄵ
            (4, 27) => Some(6),  // ㄴ + ㅎ = ㄶ
            (8, 1) => Some(9),   // ㄹ + ㄱ = ㄺ
            (8, 16) => Some(10), // ㄹ + ㅁ = ㄻ
            (8, 17) => Some(11), // ㄹ + ㅂ = ㄼ
            (8, 19) => Some(12), // ㄹ + ㅅ = ㄽ
            (8, 25) => Some(13), // ㄹ + ㅌ = ㄾ
            (8, 26) => Some(14), // ㄹ + ㅍ = ㄿ
            (8, 27) => Some(15), // ㄹ + ㅎ = ㅀ
            (17, 19) => Some(18),// ㅂ + ㅅ = ㅄ
            _ => None,
        }
    }

    /// 자음 입력(초성 인덱스 기준)을 종성 인덱스로 변환
    fn cho_to_jong(cho: u32) -> Option<u32> {
        match cho {
            0 => Some(1),   // ㄱ
            1 => Some(2),   // ㄲ
            2 => Some(4),   // ㄴ
            3 => Some(7),   // ㄷ
            5 => Some(8),   // ㄹ
            6 => Some(16),  // ㅁ
            7 => Some(17),  // ㅂ
            9 => Some(19),  // ㅅ
            10 => Some(20), // ㅆ
            11 => Some(21), // ㅇ
            12 => Some(22), // ㅈ
            14 => Some(23), // ㅊ
            15 => Some(24), // ㅋ
            16 => Some(25), // ㅌ
            17 => Some(26), // ㅍ
            18 => Some(27), // ㅎ
            _ => None,      // ㄸ, ㅃ, ㅉ는 종성에 오지 않음
        }
    }

    /// 종성 인덱스를 초성 인덱스로 변환 (단일 받침 밀어내기용)
    fn jong_to_cho(jong: u32) -> Option<u32> {
        match jong {
            1 => Some(0),   // ㄱ
            2 => Some(1),   // ㄲ
            4 => Some(2),   // ㄴ
            7 => Some(3),   // ㄷ
            8 => Some(5),   // ㄹ
            16 => Some(6),  // ㅁ
            17 => Some(7),  // ㅂ
            19 => Some(9),  // ㅅ
            20 => Some(10), // ㅆ
            21 => Some(11), // ㅇ
            22 => Some(12), // ㅈ
            23 => Some(14), // ㅊ
            24 => Some(15), // ㅋ
            25 => Some(16), // ㅌ
            26 => Some(17), // ㅍ
            27 => Some(18), // ㅎ
            _ => None,      
        }
    }

    /// 종성에 모음이 들어올 경우, 겹받침은 앞자음을 남기고 뒷자음을 초성으로, 단일 받침은 전체를 초성으로 밀어냅니다.
    fn split_complex_jong(jong: u32) -> (Option<u32>, u32) {
        match jong {
            3 => (Some(1), 9),   // ㄳ -> ㄱ(1), ㅅ(cho: 9)
            5 => (Some(4), 12),  // ㄵ -> ㄴ(4), ㅈ(cho: 12)
            6 => (Some(4), 18),  // ㄶ -> ㄴ(4), ㅎ(cho: 18)
            9 => (Some(8), 0),   // ㄺ -> ㄹ(8), ㄱ(cho: 0)
            10 => (Some(8), 6),  // ㄻ -> ㄹ(8), ㅁ(cho: 6)
            11 => (Some(8), 7),  // ㄼ -> ㄹ(8), ㅂ(cho: 7)
            12 => (Some(8), 9),  // ㄽ -> ㄹ(8), ㅅ(cho: 9)
            13 => (Some(8), 16), // ㄾ -> ㄹ(8), ㅌ(cho: 16)
            14 => (Some(8), 17), // ㄿ -> ㄹ(8), ㅍ(cho: 17)
            15 => (Some(8), 18), // ㅀ -> ㄹ(8), ㅎ(cho: 18)
            18 => (Some(17), 9), // ㅄ -> ㅂ(17), ㅅ(cho: 9)
            _ => (None, Self::jong_to_cho(jong).unwrap_or(0)), // 단일 받침은 남김없이 전부 다음 글자 초성으로 이동
        }
    }
}