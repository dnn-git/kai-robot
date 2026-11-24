//! Kai — robot trợ lý nhỏ trên màn hình (GTK4 + Cairo/Pango).

use chrono::{Datelike, Local, Timelike};
use gtk4::cairo::{Context, LineCap, LineJoin, Operator};
use gtk4::glib::{self, ControlFlow, Propagation};
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, DrawingArea, EventControllerKey, EventControllerMotion,
    GestureClick,
};
use pangocairo::functions as pango_cairo;
use rand::seq::IteratorRandom;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::f64::consts::{PI, TAU};
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

// ───────────────────────────── Hằng số ─────────────────────────────

const MIN_HEIGHT: f64 = 180.0;
const MAX_HEIGHT: f64 = 340.0;
const DEFAULT_HEIGHT: f64 = 250.0;
const MIN_EMOTION_MINUTES: u32 = 1;
const MAX_EMOTION_MINUTES: u32 = 60;
const DEFAULT_EMOTION_MINUTES: u32 = 5;
const MAX_OWNER_CHARS: usize = 32;
const DEFAULT_OWNER: &str = "Dnn";

const TICK_MS: u64 = 33;
/// Số khung hình vẽ thêm sau khi mọi thứ đã tĩnh (để chắc chắn hình cuối được vẽ).
const SETTLE_FRAMES: u8 = 3;

const IDLE_PROMPT: Duration = Duration::from_secs(15 * 60);
const IDLE_RECOVER: Duration = Duration::from_secs(10);
const IDLE_SPEECH_MS: u64 = 10_000;
const BLINK_DURATION: Duration = Duration::from_millis(140);
const GREETING_MS: u64 = 5_000;
const NIGHT_WARNING_MS: u64 = 6_000;
const GOODNIGHT_STEP: Duration = Duration::from_millis(1400);
const GOODBYE_QUIT_AFTER: Duration = Duration::from_millis(2600);
const GOODBYE_SPEECH_MS: u64 = 2800;

const NIGHT_GREETINGS: &[&str] = &[
    "Muộn rồi đấy, chủ nhân!",
    "Chủ nhân vẫn chưa ngủ sao?",
    "Hôm nay vất vả rồi nhỉ!",
    "Kai bắt đầu buồn ngủ rồi...",
    "Đêm nay yên tĩnh thật đấy.",
    "Chủ nhân nhớ nghỉ ngơi nhé!",
    "Một ngày nữa lại trôi qua rồi.",
    "Kai vẫn ở đây cùng chủ nhân!",
    "Đừng thức khuya quá nhé!",
    "Ngày mai sẽ là một ngày mới!",
];

const GOODNIGHT_LINES: &[&str] = &[
    "Chúc chủ nhân ngủ ngon!",
    "Hẹn gặp lại vào ngày mai!",
    "— Kai —",
];
/// Chuỗi ngủ: mỗi bước hiện thêm một dòng, bước cuối giữ nguyên rồi mới tắt.
const GOODNIGHT_LAST_STEP: u8 = GOODNIGHT_LINES.len() as u8 + 1;

const GOODBYE_TEXT: &str = "Chủ nhân, hẹn gặp lại!\n— Kai —";
const EXIT_PROMPT_TEXT: &str = "Chủ nhân muốn rời Kai thật sao?";
const IDLE_TEXT: &str = "Chủ nhân quên Kai rồi sao?";

const WEEKDAYS: [&str; 7] = [
    "Thứ 2", "Thứ 3", "Thứ 4", "Thứ 5", "Thứ 6", "Thứ 7", "Chủ nhật",
];

// ───────────────────────────── Thời gian / ngẫu nhiên ─────────────────────────────

/// Khuya = từ 22h đến trước 5h sáng.
fn is_night(hour: u32) -> bool {
    !(5..22).contains(&hour)
}

fn next_blink_delay() -> Duration {
    Duration::from_millis(rand::thread_rng().gen_range(2400..=5600))
}

fn next_micro_delay() -> Duration {
    Duration::from_millis(rand::thread_rng().gen_range(18_000..=38_000))
}

/// Chọn ngẫu nhiên một phần tử khác `avoid` (nếu có thể).
fn pick_different<T: Copy + PartialEq>(items: &[T], avoid: Option<T>) -> T {
    let mut rng = rand::thread_rng();
    items
        .iter()
        .copied()
        .filter(|i| Some(*i) != avoid)
        .choose(&mut rng)
        .unwrap_or_else(|| items[0])
}

/// Kích thước cửa sổ (rộng, cao) theo chiều cao cấu hình.
fn window_size(height: f64) -> (i32, i32) {
    let h = height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    ((h * 0.96).round() as i32, (h * 1.32).round() as i32)
}

// ───────────────────────────── Biểu cảm ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Emotion {
    Normal,
    Happy,
    Shy,
    Sad,
    Pout,
    Angry,
    Sleepy,
    Surprised,
    Laugh,
}

impl Emotion {
    const ALL: [Emotion; 9] = [
        Emotion::Normal,
        Emotion::Happy,
        Emotion::Shy,
        Emotion::Sad,
        Emotion::Pout,
        Emotion::Angry,
        Emotion::Sleepy,
        Emotion::Surprised,
        Emotion::Laugh,
    ];

    fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|&e| e == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Biểu cảm vui: đầu và ăng-ten lắc mạnh hơn, mắt cười.
    fn is_cheerful(self) -> bool {
        matches!(self, Emotion::Happy | Emotion::Laugh)
    }
}

// ───────────────────────────── Cấu hình ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    #[serde(default = "default_owner_name")]
    owner_name: String,
    #[serde(default = "default_height")]
    height: f64,
    #[serde(default = "default_auto_emotion")]
    auto_emotion: bool,
    #[serde(default = "default_auto_emotion_minutes")]
    auto_emotion_minutes: u32,
    #[serde(default)]
    last_active_date: String,
}

fn default_owner_name() -> String {
    DEFAULT_OWNER.to_string()
}
fn default_height() -> f64 {
    DEFAULT_HEIGHT
}
fn default_auto_emotion() -> bool {
    true
}
fn default_auto_emotion_minutes() -> u32 {
    DEFAULT_EMOTION_MINUTES
}

impl Default for Config {
    fn default() -> Self {
        Self {
            owner_name: default_owner_name(),
            height: default_height(),
            auto_emotion: default_auto_emotion(),
            auto_emotion_minutes: default_auto_emotion_minutes(),
            last_active_date: String::new(),
        }
    }
}

impl Config {
    /// Đưa mọi giá trị về khoảng hợp lệ (file config có thể bị sửa tay / hỏng).
    fn normalized(mut self) -> Self {
        if !self.height.is_finite() {
            self.height = DEFAULT_HEIGHT;
        }
        self.height = self.height.clamp(MIN_HEIGHT, MAX_HEIGHT);
        self.auto_emotion_minutes = self
            .auto_emotion_minutes
            .clamp(MIN_EMOTION_MINUTES, MAX_EMOTION_MINUTES);

        let owner: String = self.owner_name.trim().chars().take(MAX_OWNER_CHARS).collect();
        let owner = owner.trim();
        self.owner_name = if owner.is_empty() {
            default_owner_name()
        } else {
            owner.to_string()
        };
        self
    }
}

fn config_path() -> PathBuf {
    glib::user_config_dir().join("kai").join("config.json")
}

fn load_config() -> Config {
    let path = config_path();
    let Ok(raw) = fs::read_to_string(&path) else {
        return Config::default();
    };
    match serde_json::from_str::<Config>(&raw) {
        Ok(cfg) => cfg.normalized(),
        Err(err) => {
            eprintln!("kai: config.json không hợp lệ ({err}), dùng giá trị mặc định");
            // Giữ lại bản hỏng để người dùng còn xem được.
            let _ = fs::rename(&path, path.with_extension("json.bad"));
            Config::default()
        }
    }
}

/// Ghi an toàn: ghi ra file tạm rồi đổi tên, tránh để lại file nửa chừng.
fn save_config(cfg: &Config) {
    let path = config_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if let Err(err) = fs::create_dir_all(parent) {
        eprintln!("kai: không tạo được thư mục config: {err}");
        return;
    }
    let json = match serde_json::to_string_pretty(cfg) {
        Ok(json) => json,
        Err(err) => {
            eprintln!("kai: không tạo được JSON config: {err}");
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(err) = fs::write(&tmp, json).and_then(|_| fs::rename(&tmp, &path)) {
        eprintln!("kai: không ghi được config: {err}");
    }
}

// ───────────────────────────── Kiểu dùng chung ─────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenMode {
    Face,
    Clock,
    Stopwatch,
    ExitConfirm,
    Goodbye,
    Goodnight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MicroBehavior {
    None,
    Thinking,
    Yawn,
    LeftWave,
}

/// Các vùng bấm được trên màn hình.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HitKey {
    On,
    Off,
    Emotion,
    Clock,
    Exit,
    Settings,
    ExitStay,
    ExitYes,
    SwStart,
    SwStop,
    SwReset,
}

struct TickOutcome {
    quit: bool,
    redraw: bool,
}

// ───────────────────────────── Trạng thái ứng dụng ─────────────────────────────

struct AppState {
    config: Config,
    is_on: bool,
    screen_mode: ScreenMode,
    prev_mode_before_exit: ScreenMode,
    emotion: Emotion,
    is_blinking: bool,
    blink_until: Option<Instant>,
    next_blink_at: Instant,
    micro: MicroBehavior,
    micro_until: Option<Instant>,
    next_micro_at: Instant,
    late_night_warned: bool,
    last_night_greeting: Option<&'static str>,
    speech_text: String,
    speech_until: Option<Instant>,
    goodnight_step: u8,
    goodnight_next_at: Option<Instant>,
    goodbye_quit_at: Option<Instant>,
    idle_recover_at: Option<Instant>,
    last_interaction: Instant,
    idle_prompted: bool,
    last_auto_emotion: Instant,
    t_phase: f64,
    head_bob: f64,
    antenna_angle: f64,
    arm_angle: f64,
    left_arm_raise: f64,
    foot_bob: f64,
    brightness_factor: f64,
    sw_running: bool,
    sw_elapsed_ms: u64,
    sw_last_tick: Option<Instant>,
    hit_rects: Vec<(HitKey, Rect)>,
    hover_key: Option<HitKey>,
    settle_frames: u8,
}

impl AppState {
    fn new() -> Self {
        let now = Instant::now();
        let now_dt = Local::now();
        let hour = now_dt.hour();
        let today = now_dt.format("%Y-%m-%d").to_string();

        let mut cfg = load_config();
        let is_new_day = cfg.last_active_date != today;
        if is_new_day {
            cfg.last_active_date = today;
            save_config(&cfg);
        }

        let mut st = Self {
            config: cfg,
            is_on: true,
            screen_mode: ScreenMode::Face,
            prev_mode_before_exit: ScreenMode::Face,
            emotion: Emotion::Happy,
            is_blinking: false,
            blink_until: None,
            next_blink_at: now + next_blink_delay(),
            micro: MicroBehavior::None,
            micro_until: None,
            next_micro_at: now + next_micro_delay(),
            late_night_warned: is_night(hour),
            last_night_greeting: None,
            speech_text: String::new(),
            speech_until: None,
            goodnight_step: 0,
            goodnight_next_at: None,
            goodbye_quit_at: None,
            idle_recover_at: None,
            last_interaction: now,
            idle_prompted: false,
            last_auto_emotion: now,
            t_phase: 0.0,
            head_bob: 0.0,
            antenna_angle: 0.0,
            arm_angle: 0.0,
            left_arm_raise: 0.0,
            foot_bob: 0.0,
            brightness_factor: 1.0,
            sw_running: false,
            sw_elapsed_ms: 0,
            sw_last_tick: None,
            hit_rects: Vec::new(),
            hover_key: None,
            settle_frames: 0,
        };

        // Khuya thì ưu tiên lời chào khuya; ngày mới thì tự giới thiệu.
        let (text, ms) = if !is_night(hour) && is_new_day {
            (
                format!(
                    "Xin chào! Tôi là Kai. Chủ nhân của tôi là {}.",
                    st.config.owner_name
                ),
                5_000,
            )
        } else {
            (st.greeting_line(hour), 6_000)
        };
        st.show_speech(text, ms);
        st
    }

    // ── Tiện ích trạng thái ──

    /// Đang ở màn hình "chuyển cảnh": không cho bấm các nút điều khiển thường.
    fn input_locked(&self) -> bool {
        matches!(
            self.screen_mode,
            ScreenMode::ExitConfirm | ScreenMode::Goodbye | ScreenMode::Goodnight
        )
    }

    /// Nút "On" vẫn dùng được trong lúc chuỗi ngủ (để huỷ), nhưng không khi hỏi thoát / tạm biệt.
    fn on_allowed(&self) -> bool {
        !matches!(
            self.screen_mode,
            ScreenMode::ExitConfirm | ScreenMode::Goodbye
        )
    }

    fn settings_allowed(&self) -> bool {
        self.on_allowed()
    }

    fn reset_idle_timer(&mut self) {
        self.last_interaction = Instant::now();
        self.idle_prompted = false;
        self.idle_recover_at = None;
    }

    fn restart_auto_emotion_timer(&mut self) {
        self.last_auto_emotion = Instant::now();
    }

    fn set_emotion(&mut self, emotion: Emotion) {
        self.emotion = emotion;
    }

    fn clear_micro(&mut self) {
        self.micro = MicroBehavior::None;
        self.micro_until = None;
    }

    fn show_speech(&mut self, text: impl Into<String>, duration_ms: u64) {
        self.speech_text = text.into();
        self.speech_until = if duration_ms > 0 {
            Some(Instant::now() + Duration::from_millis(duration_ms))
        } else {
            None
        };
    }

    fn clear_speech(&mut self) {
        self.speech_text.clear();
        self.speech_until = None;
    }

    fn night_greeting(&mut self) -> String {
        let line = pick_different(NIGHT_GREETINGS, self.last_night_greeting);
        self.last_night_greeting = Some(line);
        line.to_string()
    }

    fn greeting_line(&mut self, hour: u32) -> String {
        match hour {
            h if is_night(h) => self.night_greeting(),
            5..=10 => "Chào buổi sáng, chủ nhân!".to_string(),
            11..=13 => "Chào buổi trưa, chủ nhân!".to_string(),
            14..=17 => "Chào buổi chiều, chủ nhân!".to_string(),
            _ => "Chào buổi tối, chủ nhân!".to_string(),
        }
    }

    fn enter_sleep_state(&mut self) {
        self.is_on = false;
        self.screen_mode = ScreenMode::Face;
        self.goodnight_step = 0;
        self.goodnight_next_at = None;
        self.clear_micro();
        self.clear_speech();
    }

    // ── Hành động người dùng ──

    fn action_on(&mut self) {
        if !self.on_allowed() || (self.is_on && self.screen_mode != ScreenMode::Goodnight) {
            return;
        }
        self.goodnight_step = 0;
        self.goodnight_next_at = None;
        self.is_on = true;
        self.screen_mode = ScreenMode::Face;
        self.set_emotion(Emotion::Happy);
        self.reset_idle_timer();
        self.restart_auto_emotion_timer();

        let line = self.greeting_line(Local::now().hour());
        self.show_speech(line, GREETING_MS);
    }

    fn action_off(&mut self) {
        if !self.is_on || self.input_locked() {
            return;
        }
        self.idle_prompted = false;
        self.idle_recover_at = None;
        self.clear_micro();
        self.clear_speech();

        if is_night(Local::now().hour()) {
            self.screen_mode = ScreenMode::Goodnight;
            self.goodnight_step = 1;
            self.goodnight_next_at = Some(Instant::now() + GOODNIGHT_STEP);
        } else {
            self.enter_sleep_state();
        }
    }

    fn action_emotion(&mut self) {
        if !self.is_on || self.input_locked() {
            return;
        }
        self.reset_idle_timer();
        if matches!(self.screen_mode, ScreenMode::Clock | ScreenMode::Stopwatch) {
            self.screen_mode = ScreenMode::Face;
            self.set_emotion(Emotion::Normal);
            self.restart_auto_emotion_timer();
            return;
        }
        self.clear_micro();
        self.set_emotion(self.emotion.next());
        self.restart_auto_emotion_timer();
    }

    fn action_clock(&mut self) {
        if !self.is_on || self.input_locked() {
            return;
        }
        self.reset_idle_timer();
        self.clear_micro();
        self.screen_mode = match self.screen_mode {
            ScreenMode::Clock => ScreenMode::Stopwatch,
            _ => ScreenMode::Clock,
        };
    }

    fn action_exit_prompt(&mut self) {
        if matches!(
            self.screen_mode,
            ScreenMode::ExitConfirm | ScreenMode::Goodbye
        ) {
            return;
        }
        self.prev_mode_before_exit = self.screen_mode;
        self.screen_mode = ScreenMode::ExitConfirm;
        self.show_speech(EXIT_PROMPT_TEXT, 0);
    }

    fn action_exit_stay(&mut self) {
        if self.screen_mode != ScreenMode::ExitConfirm {
            return;
        }
        self.screen_mode = self.prev_mode_before_exit;
        self.clear_speech();
        if self.is_on {
            self.reset_idle_timer();
        }
    }

    fn action_exit_confirm(&mut self) {
        if self.screen_mode != ScreenMode::ExitConfirm {
            return;
        }
        self.show_speech(GOODBYE_TEXT, GOODBYE_SPEECH_MS);
        self.screen_mode = ScreenMode::Goodbye;
        self.goodbye_quit_at = Some(Instant::now() + GOODBYE_QUIT_AFTER);
    }

    // ── Đồng hồ bấm giờ ──

    fn current_stopwatch_ms(&self) -> u64 {
        match (self.sw_running, self.sw_last_tick) {
            (true, Some(prev)) => {
                let delta = Instant::now().saturating_duration_since(prev).as_millis() as u64;
                self.sw_elapsed_ms.saturating_add(delta)
            }
            _ => self.sw_elapsed_ms,
        }
    }

    fn sw_start(&mut self) {
        if !self.sw_running {
            self.sw_running = true;
            self.sw_last_tick = Some(Instant::now());
        }
    }

    fn sw_pause(&mut self) {
        if self.sw_running {
            self.sw_elapsed_ms = self.current_stopwatch_ms();
            self.sw_last_tick = None;
            self.sw_running = false;
        }
    }

    fn sw_reset(&mut self) {
        self.sw_running = false;
        self.sw_elapsed_ms = 0;
        self.sw_last_tick = None;
    }

    // ── Vòng lặp cập nhật (gọi mỗi ~33 ms) ──

    fn tick(&mut self, now: Instant, hour: u32) -> TickOutcome {
        let quit = self.goodbye_quit_at.is_some_and(|t| now >= t);

        if self.screen_mode != ScreenMode::ExitConfirm
            && self.speech_until.is_some_and(|t| now >= t)
        {
            self.clear_speech();
        }

        self.tick_goodnight(now);
        self.tick_blink(now);
        self.tick_micro(now);
        self.tick_night_warning(hour);
        self.tick_idle(now);
        self.tick_auto_emotion(now);
        self.tick_animation();

        TickOutcome {
            quit,
            redraw: self.needs_redraw(),
        }
    }

    fn tick_goodnight(&mut self, now: Instant) {
        if self.screen_mode != ScreenMode::Goodnight {
            return;
        }
        let Some(next_at) = self.goodnight_next_at else {
            return;
        };
        if now < next_at {
            return;
        }
        self.goodnight_step += 1;
        if self.goodnight_step > GOODNIGHT_LAST_STEP {
            self.enter_sleep_state();
        } else {
            self.goodnight_next_at = Some(now + GOODNIGHT_STEP);
        }
    }

    fn tick_blink(&mut self, now: Instant) {
        if self.is_blinking {
            if self.blink_until.map_or(true, |t| now >= t) {
                self.is_blinking = false;
                self.blink_until = None;
            }
        } else if now >= self.next_blink_at {
            if self.is_on && self.screen_mode == ScreenMode::Face {
                self.is_blinking = true;
                self.blink_until = Some(now + BLINK_DURATION);
            }
            self.next_blink_at = now + next_blink_delay();
        }
    }

    fn tick_micro(&mut self, now: Instant) {
        if self.micro != MicroBehavior::None {
            if self.micro_until.map_or(true, |t| now >= t) {
                self.clear_micro();
            }
        } else if now >= self.next_micro_at {
            if self.is_on && self.screen_mode == ScreenMode::Face && self.speech_text.is_empty() {
                let roll = rand::thread_rng().gen_range(0..10);
                let (kind, ms) = if roll < 4 {
                    (MicroBehavior::Thinking, 3500)
                } else if roll < 8 {
                    (MicroBehavior::Yawn, 2500)
                } else {
                    (MicroBehavior::LeftWave, 2800)
                };
                self.micro = kind;
                self.micro_until = Some(now + Duration::from_millis(ms));
            }
            self.next_micro_at = now + next_micro_delay();
        }
    }

    fn tick_night_warning(&mut self, hour: u32) {
        if !is_night(hour) {
            self.late_night_warned = false;
        } else if self.is_on && !self.late_night_warned && self.speech_text.is_empty() {
            self.late_night_warned = true;
            let line = self.night_greeting();
            self.show_speech(line, NIGHT_WARNING_MS);
        }
    }

    fn tick_idle(&mut self, now: Instant) {
        if self.is_on
            && self.screen_mode == ScreenMode::Face
            && !self.idle_prompted
            && now.saturating_duration_since(self.last_interaction) >= IDLE_PROMPT
        {
            self.idle_prompted = true;
            self.clear_micro();
            self.set_emotion(Emotion::Pout);
            self.show_speech(IDLE_TEXT, IDLE_SPEECH_MS);
            self.idle_recover_at = Some(now + IDLE_RECOVER);
        }

        if let Some(rec_at) = self.idle_recover_at {
            if now >= rec_at {
                self.idle_recover_at = None;
                if self.is_on
                    && self.screen_mode == ScreenMode::Face
                    && self.emotion == Emotion::Pout
                {
                    let chosen =
                        pick_different(&[Emotion::Normal, Emotion::Happy, Emotion::Sleepy], None);
                    self.set_emotion(chosen);
                }
            }
        }
    }

    fn tick_auto_emotion(&mut self, now: Instant) {
        if !(self.is_on && self.screen_mode == ScreenMode::Face && self.config.auto_emotion) {
            return;
        }
        let minutes = self.config.auto_emotion_minutes.max(1) as u64;
        let interval = Duration::from_secs(minutes * 60);
        if now.saturating_duration_since(self.last_auto_emotion) >= interval {
            self.last_auto_emotion = now;
            let chosen = pick_different(&Emotion::ALL, Some(self.emotion));
            self.set_emotion(chosen);
        }
    }

    fn tick_animation(&mut self) {
        self.t_phase += 0.08;
        let target_b = if self.is_on { 1.0 } else { 0.70 };
        self.brightness_factor += (target_b - self.brightness_factor) * 0.15;

        let target_raise = if self.is_on && self.micro == MicroBehavior::LeftWave {
            55.0
        } else {
            0.0
        };
        self.left_arm_raise += (target_raise - self.left_arm_raise) * 0.14;

        if self.is_on {
            let cheerful = self.emotion.is_cheerful();
            let amp = if cheerful { 1.6 } else { 0.7 };
            let ant_amp = if cheerful { 8.0 } else { 3.5 };
            self.head_bob = (self.t_phase * 1.1).sin() * amp;
            self.antenna_angle = (self.t_phase * 1.7 + 0.5).sin() * ant_amp;
            self.arm_angle = (self.t_phase * 0.9 + 1.2).sin() * 4.0;
            self.foot_bob = (self.t_phase * 1.1).cos() * 0.8;
        } else {
            self.head_bob = 2.5;
            self.antenna_angle = -14.0;
            self.arm_angle = 6.0;
            self.foot_bob = 0.0;
        }
    }

    /// Chỉ vẽ lại khi có gì đang chuyển động; lúc Kai ngủ yên thì không tốn CPU.
    fn needs_redraw(&mut self) -> bool {
        let target_b = if self.is_on { 1.0 } else { 0.70 };
        let active = self.is_on
            || self.screen_mode != ScreenMode::Face
            || !self.speech_text.is_empty()
            || (self.brightness_factor - target_b).abs() > 0.002
            || self.left_arm_raise.abs() > 0.05;

        if active {
            self.settle_frames = 0;
            true
        } else if self.settle_frames < SETTLE_FRAMES {
            self.settle_frames += 1;
            true
        } else {
            false
        }
    }
}

// ───────────────────────────── Tiện ích vẽ ─────────────────────────────

type Rgba = (f64, f64, f64, f64);

/// Màu cố định (không bị làm tối theo độ sáng).
fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Rgba {
    (
        r as f64 / 255.0,
        g as f64 / 255.0,
        b as f64 / 255.0,
        a as f64 / 255.0,
    )
}

/// Màu có nhân độ sáng `brightness` (dùng khi Kai tắt thì tối đi).
fn shade_rgba(r: u8, g: u8, b: u8, a: f64, brightness: f64) -> Rgba {
    let ch = |v: u8| ((v as f64 * brightness).clamp(0.0, 255.0)) / 255.0;
    (ch(r), ch(g), ch(b), a.clamp(0.0, 1.0))
}

fn set_color(cr: &Context, c: Rgba) {
    cr.set_source_rgba(c.0, c.1, c.2, c.3);
}

fn fill_color(cr: &Context, c: Rgba) {
    set_color(cr, c);
    let _ = cr.fill();
}

fn stroke_color(cr: &Context, c: Rgba, line_w: f64) {
    set_color(cr, c);
    cr.set_line_width(line_w);
    let _ = cr.stroke();
}

fn fill_stroke(cr: &Context, fill: Rgba, stroke: Rgba, line_w: f64) {
    set_color(cr, fill);
    let _ = cr.fill_preserve();
    stroke_color(cr, stroke, line_w);
}

fn sans(pt: f64) -> String {
    format!("Sans {pt:.1}")
}

fn sans_bold(pt: f64) -> String {
    format!("Sans Bold {pt:.1}")
}

fn draw_rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let radius = r.min(w * 0.5).min(h * 0.5).max(0.0);
    cr.new_sub_path();
    cr.arc(x + w - radius, y + radius, radius, -PI * 0.5, 0.0);
    cr.arc(x + w - radius, y + h - radius, radius, 0.0, PI * 0.5);
    cr.arc(x + radius, y + h - radius, radius, PI * 0.5, PI);
    cr.arc(x + radius, y + radius, radius, PI, 1.5 * PI);
    cr.close_path();
}

/// Đường cong bậc 2 từ `p0` qua điểm điều khiển `q` đến `p1` (Cairo chỉ có bậc 3).
fn quad_to(cr: &Context, p0: (f64, f64), q: (f64, f64), p1: (f64, f64)) {
    cr.curve_to(
        p0.0 + 2.0 / 3.0 * (q.0 - p0.0),
        p0.1 + 2.0 / 3.0 * (q.1 - p0.1),
        p1.0 + 2.0 / 3.0 * (q.0 - p1.0),
        p1.1 + 2.0 / 3.0 * (q.1 - p1.1),
        p1.0,
        p1.1,
    );
}

fn draw_ellipse(cr: &Context, x: f64, y: f64, rx: f64, ry: f64, color: Rgba) {
    cr.new_path();
    let _ = cr.save();
    cr.translate(x, y);
    cr.scale(rx, ry);
    cr.arc(0.0, 0.0, 1.0, 0.0, TAU);
    let _ = cr.restore();
    fill_color(cr, color);
}

fn draw_rivet(cr: &Context, x: f64, y: f64, r: f64, fill: Rgba, stroke: Rgba, line_w: f64) {
    cr.new_sub_path();
    cr.arc(x, y, r, 0.0, TAU);
    fill_stroke(cr, fill, stroke, line_w);
}

fn make_layout(
    cr: &Context,
    text: &str,
    font: &str,
    wrap_w: Option<f64>,
) -> gtk4::pango::Layout {
    let layout = pango_cairo::create_layout(cr);
    let desc = gtk4::pango::FontDescription::from_string(font);
    layout.set_font_description(Some(&desc));
    layout.set_text(text);
    if let Some(w) = wrap_w {
        layout.set_width((w * gtk4::pango::SCALE as f64).max(1.0) as i32);
        layout.set_wrap(gtk4::pango::WrapMode::WordChar);
    }
    layout
}

/// Vẽ chữ căn giữa tại (x, y). `wrap_w = Some(w)` sẽ tự xuống dòng theo chiều rộng w.
fn draw_text_centered(
    cr: &Context,
    text: &str,
    font: &str,
    x: f64,
    y: f64,
    wrap_w: Option<f64>,
    color: Rgba,
) {
    let layout = make_layout(cr, text, font, wrap_w);
    layout.set_alignment(gtk4::pango::Alignment::Center);
    let (ink, logical) = layout.pixel_extents();
    let ink_center_x = ink.x() as f64 + ink.width() as f64 * 0.5;
    let lh = logical.height() as f64;

    let draw_x = x - ink_center_x;
    let draw_y = y - (lh * 0.5 - (ink.y() as f64).min(0.0) * 0.2);

    set_color(cr, color);
    cr.move_to(draw_x, draw_y);
    pango_cairo::show_layout(cr, &layout);
}

fn measure_text(cr: &Context, text: &str, font: &str, wrap_w: Option<f64>) -> (f64, f64) {
    let layout = make_layout(cr, text, font, wrap_w);
    let (_, logical) = layout.pixel_extents();
    (logical.width() as f64, logical.height() as f64)
}

#[allow(clippy::too_many_arguments)]
fn draw_simple_button(
    cr: &Context,
    rect: Rect,
    label: &str,
    fill_rgb: (u8, u8, u8),
    border_rgb: (u8, u8, u8),
    txt_color: Rgba,
    font_pt: f64,
    radius: f64,
    b: f64,
) {
    let fill = shade_rgba(fill_rgb.0, fill_rgb.1, fill_rgb.2, 1.0, b);
    let border = shade_rgba(border_rgb.0, border_rgb.1, border_rgb.2, 1.0, b);
    draw_rounded_rect(cr, rect.x, rect.y, rect.w, rect.h, radius);
    fill_stroke(cr, fill, border, 1.0);
    draw_text_centered(
        cr,
        label,
        &sans_bold(font_pt),
        rect.x + rect.w * 0.5,
        rect.y + rect.h * 0.5,
        None,
        txt_color,
    );
}

/// Hộp bao (axis-aligned) của một hình chữ nhật sau khi xoay quanh `pivot`.
fn rotated_bounds(pivot_x: f64, pivot_y: f64, local: [(f64, f64); 4], angle_deg: f64) -> Rect {
    let (s, c) = angle_deg.to_radians().sin_cos();
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for (px, py) in local {
        let wx = pivot_x + px * c - py * s;
        let wy = pivot_y + px * s + py * c;
        min_x = min_x.min(wx);
        min_y = min_y.min(wy);
        max_x = max_x.max(wx);
        max_y = max_y.max(wy);
    }
    Rect {
        x: min_x,
        y: min_y,
        w: max_x - min_x,
        h: max_y - min_y,
    }
}

// ───────────────────────────── Các chi tiết khuôn mặt dùng chung ─────────────────────────────

/// Mắt cười "^ ^".
fn draw_happy_eyes(cr: &Context, cx: f64, y: f64, dx: f64, r: f64, color: Rgba, line_w: f64) {
    cr.set_line_cap(LineCap::Round);
    for side in [-1.0, 1.0] {
        cr.new_sub_path();
        cr.arc(cx + side * dx, y + r * 0.15, r, PI * 1.08, PI * 1.92);
        stroke_color(cr, color, line_w);
    }
}

/// Miệng cười (màu và độ dày nét đã được đặt sẵn).
fn draw_smile(cr: &Context, cx: f64, mouth_y: f64, sw: f64, sh: f64) {
    cr.new_sub_path();
    cr.arc(cx, mouth_y - sh * 0.02, sw * 0.09, PI * 0.08, PI * 0.92);
    let _ = cr.stroke();
}

/// Hai má hồng trên mặt.
fn draw_face_blush(cr: &Context, cx: f64, cy: f64, sw: f64, sh: f64, color: Rgba) {
    let bw = sw * 0.15;
    let bh = sh * 0.09;
    for bx in [cx - sw * 0.34 + bw * 0.5, cx + sw * 0.19 + bw * 0.5] {
        draw_ellipse(cr, bx, cy + sh * 0.08 + bh * 0.5, bw * 0.5, bh * 0.5, color);
    }
}

fn draw_star(cr: &Context, sx: f64, sy: f64, r: f64) {
    let k = 0.22;
    cr.new_path();
    cr.move_to(sx, sy - r);
    quad_to(cr, (sx, sy - r), (sx + r * k, sy - r * k), (sx + r, sy));
    quad_to(cr, (sx + r, sy), (sx + r * k, sy + r * k), (sx, sy + r));
    quad_to(cr, (sx, sy + r), (sx - r * k, sy + r * k), (sx - r, sy));
    quad_to(cr, (sx - r, sy), (sx - r * k, sy - r * k), (sx, sy - r));
    cr.close_path();
    let _ = cr.fill();
}

// ───────────────────────────── Màn hình khuôn mặt ─────────────────────────────

fn draw_face_expression(cr: &Context, scr: Rect, state: &AppState) {
    let _ = cr.save();
    cr.rectangle(scr.x, scr.y, scr.w, scr.h);
    cr.clip();

    if state.is_on {
        draw_awake_face(cr, scr, state);
    } else {
        draw_off_face(cr, scr, state.brightness_factor);
    }
    let _ = cr.restore();
}

/// Khi tắt: trăng khuyết, sao và "Zz".
fn draw_off_face(cr: &Context, scr: Rect, b: f64) {
    let cx = scr.x + scr.w * 0.5;
    let cy = scr.y + scr.h * 0.5;
    let (sw, sh) = (scr.w, scr.h);

    let moon = shade_rgba(0xD9, 0xE0, 0xE5, 1.0, b);
    let star = shade_rgba(0xAA, 0xB8, 0xC2, 1.0, b);
    let bg = shade_rgba(0x20, 0x29, 0x32, 1.0, b);
    let radius = sw.min(sh) * 0.205;
    let mx = cx - sw * 0.13;
    let my = cy - sh * 0.07;

    cr.new_path();
    cr.arc(mx, my, radius, 0.0, TAU);
    fill_color(cr, moon);

    cr.new_path();
    cr.arc(mx + radius * 0.42, my - radius * 0.48, radius * 0.92, 0.0, TAU);
    fill_color(cr, bg);

    set_color(cr, star);
    draw_star(cr, cx - sw * 0.36, cy - sh * 0.29, 4.0);
    draw_star(cr, cx + sw * 0.29, cy - sh * 0.30, 3.2);

    draw_text_centered(
        cr,
        "Zz",
        &sans((sh * 0.18).max(11.0)),
        cx + sw * 0.22,
        cy + sh * 0.22,
        None,
        moon,
    );
}

fn draw_awake_face(cr: &Context, scr: Rect, state: &AppState) {
    let cx = scr.x + scr.w * 0.5;
    let cy = scr.y + scr.h * 0.5;
    let (sw, sh) = (scr.w, scr.h);
    let b = state.brightness_factor;

    // Khi đang "suy nghĩ" thì không áp dụng biểu cảm nào (None).
    let thinking = state.micro == MicroBehavior::Thinking;
    let emo: Option<Emotion> = if thinking { None } else { Some(state.emotion) };
    let is = |e: Emotion| emo == Some(e);

    if matches!(emo, Some(Emotion::Happy | Emotion::Shy | Emotion::Laugh)) {
        let alpha = if is(Emotion::Happy) { 85 } else { 150 };
        draw_face_blush(cr, cx, cy, sw, sh, rgba8(170, 181, 188, alpha));
    }

    let eye_dx = sw * 0.22;
    let eye_y = cy - sh * 0.06;
    let eye_r = sw.min(sh) * 0.13;
    let eye_line = (sw * 0.025).max(2.0);
    let dark = shade_rgba(0xF2, 0xF5, 0xF6, 1.0, b);

    cr.set_line_cap(LineCap::Round);
    cr.set_line_join(LineJoin::Round);

    // ── Mắt ──
    if state.is_blinking || is(Emotion::Sleepy) {
        for side in [-1.0, 1.0] {
            let ex = cx + side * eye_dx;
            if is(Emotion::Sleepy) {
                cr.new_sub_path();
                cr.arc(ex, eye_y, eye_r, 0.0, PI);
                cr.close_path();
                fill_color(cr, dark);
                cr.move_to(ex - eye_r, eye_y);
                cr.line_to(ex + eye_r, eye_y);
                stroke_color(cr, dark, eye_line);
            } else {
                cr.new_sub_path();
                cr.arc(
                    ex,
                    eye_y - eye_r * 0.4 + eye_r * 0.55,
                    eye_r,
                    PI * 1.05,
                    PI * 1.95,
                );
                stroke_color(cr, dark, eye_line);
            }
        }
    } else if emo.is_some_and(Emotion::is_cheerful) {
        draw_happy_eyes(cr, cx, eye_y, eye_dx, eye_r, dark, eye_line);
    } else {
        let (look_x, look_y) = if thinking {
            (sw * 0.05, -sh * 0.04)
        } else {
            (0.0, 0.0)
        };
        let r = if is(Emotion::Surprised) {
            eye_r * 1.18
        } else {
            eye_r
        };
        let shine = rgba8(170, 184, 193, 255);
        let shine_r = (eye_r * 0.34).max(2.0);
        for side in [-1.0, 1.0] {
            let ex = cx + side * eye_dx + look_x;
            let ey = eye_y + look_y;
            cr.new_sub_path();
            cr.arc(ex, ey, r, 0.0, TAU);
            fill_color(cr, dark);
            cr.new_sub_path();
            cr.arc(ex - eye_r * 0.32, ey - eye_r * 0.32, shine_r, 0.0, TAU);
            fill_color(cr, shine);
        }
    }

    // ── Lông mày ──
    let brow_y = eye_y - eye_r * 1.45;
    set_color(cr, dark);
    cr.set_line_width((sw * 0.022).max(1.8));
    for side in [-1.0, 1.0] {
        let bx = cx + side * eye_dx;
        match emo {
            Some(Emotion::Angry) => {
                cr.move_to(bx - side * eye_r * 0.8, brow_y + 3.0);
                cr.line_to(bx + side * eye_r * 0.8, brow_y - 3.0);
                let _ = cr.stroke();
            }
            Some(Emotion::Sad | Emotion::Pout) => {
                cr.move_to(bx - side * eye_r * 0.8, brow_y - 3.0);
                cr.line_to(bx + side * eye_r * 0.8, brow_y + 2.0);
                let _ = cr.stroke();
            }
            Some(Emotion::Surprised) => {
                cr.new_sub_path();
                cr.arc(bx, brow_y - 1.5, eye_r * 0.7, PI * 1.08, PI * 1.92);
                let _ = cr.stroke();
            }
            _ => {}
        }
    }

    // ── Miệng ──
    let mouth_y = cy + sh * 0.19;
    set_color(cr, dark);
    cr.set_line_width((sw * 0.024).max(1.8));
    if state.micro == MicroBehavior::Yawn || is(Emotion::Surprised) {
        cr.new_path();
        let _ = cr.save();
        cr.translate(cx, mouth_y);
        cr.scale(sw * 0.065, sh * 0.075);
        cr.arc(0.0, 0.0, 1.0, 0.0, TAU);
        let _ = cr.restore();
        let _ = cr.fill();
    } else if is(Emotion::Laugh) {
        cr.new_path();
        let _ = cr.save();
        cr.translate(cx, mouth_y - sh * 0.06 + sh * 0.075);
        cr.scale(sw * 0.11, sh * 0.075);
        cr.arc(0.0, 0.0, 1.0, 0.0, PI);
        let _ = cr.restore();
        cr.close_path();
        let _ = cr.fill();
    } else if is(Emotion::Happy) || is(Emotion::Shy) {
        draw_smile(cr, cx, mouth_y, sw, sh);
    } else if matches!(
        emo,
        Some(Emotion::Sad | Emotion::Pout | Emotion::Angry)
    ) {
        cr.new_sub_path();
        cr.arc(cx, mouth_y + sh * 0.045, sw * 0.08, PI * 1.08, PI * 1.92);
        let _ = cr.stroke();
    } else {
        cr.move_to(cx - sw * 0.06, mouth_y);
        cr.line_to(cx + sw * 0.06, mouth_y);
        let _ = cr.stroke();
    }
}

// ───────────────────────────── Các màn hình phụ ─────────────────────────────

fn draw_clock_screen(cr: &Context, scr: Rect, b: f64) {
    let now = Local::now();
    let wd_idx = now.weekday().num_days_from_monday() as usize;
    let day_str = format!(
        "{}, {:02}/{:02}",
        WEEKDAYS[wd_idx.min(6)],
        now.day(),
        now.month()
    );
    let time_str = format!("{:02}:{:02}", now.hour(), now.minute());

    draw_text_centered(
        cr,
        &time_str,
        &sans_bold((scr.h * 0.24).max(12.0)),
        scr.x + scr.w * 0.5,
        scr.y + scr.h * 0.36,
        None,
        shade_rgba(0xE6, 0xF5, 0xFA, 1.0, b),
    );
    draw_text_centered(
        cr,
        &day_str,
        &sans((scr.h * 0.11).max(7.5)),
        scr.x + scr.w * 0.5,
        scr.y + scr.h * 0.74,
        None,
        shade_rgba(0xA8, 0xC6, 0xD5, 1.0, b),
    );
}

fn format_stopwatch(ms: u64) -> String {
    let total_sec = ms / 1000;
    let secs = total_sec % 60;
    if total_sec >= 3600 {
        // Từ 1 giờ trở lên: H:MM:SS (bỏ phần trăm giây cho vừa màn hình).
        let hours = (total_sec / 3600).min(99);
        let mins = (total_sec / 60) % 60;
        format!("{hours}:{mins:02}:{secs:02}")
    } else {
        let mins = total_sec / 60;
        let centis = (ms % 1000) / 10;
        format!("{mins:02}:{secs:02}.{centis:02}")
    }
}

fn draw_stopwatch_screen(cr: &Context, scr: Rect, state: &mut AppState) {
    let b = state.brightness_factor;
    let sw_str = format_stopwatch(state.current_stopwatch_ms());

    let timer_pt = (scr.h * 0.165).clamp(9.5, 14.0);
    draw_text_centered(
        cr,
        &sw_str,
        &format!("Monospace Bold {timer_pt:.1}"),
        scr.x + scr.w * 0.5,
        scr.y + scr.h * 0.29,
        None,
        shade_rgba(0xE6, 0xF5, 0xFA, 1.0, b),
    );

    let pad_x = 4.0;
    let gap = 2.5;
    let btn_h = (scr.h * 0.24).clamp(15.0, 21.0);
    let btn_y = scr.y + scr.h - btn_h - 4.0;
    let bw = ((scr.w - pad_x * 2.0 - gap * 2.0) / 3.0).max(12.0);
    let btn_pt = (btn_h * 0.36).clamp(7.0, 9.5);

    let items: [(HitKey, &str, (u8, u8, u8)); 3] = [
        (HitKey::SwStart, "Chạy", (0x3E, 0x9A, 0x83)),
        (HitKey::SwStop, "Dừng", (0xBB, 0x69, 0x71)),
        (HitKey::SwReset, "Xóa", (0x8A, 0x96, 0x9F)),
    ];

    for (idx, (key, label, rgb)) in items.into_iter().enumerate() {
        let rect = Rect {
            x: scr.x + pad_x + idx as f64 * (bw + gap),
            y: btn_y,
            w: bw,
            h: btn_h,
        };
        state.hit_rects.push((key, rect));
        draw_simple_button(
            cr,
            rect,
            label,
            rgb,
            (0x53, 0x61, 0x6C),
            (1.0, 1.0, 1.0, 1.0),
            btn_pt,
            3.5,
            b,
        );
    }
}

fn draw_exit_confirm_screen(cr: &Context, scr: Rect, state: &mut AppState) {
    let b = state.brightness_factor;

    draw_text_centered(
        cr,
        "?",
        &sans_bold((scr.h * 0.28).max(13.0)),
        scr.x + scr.w * 0.5,
        scr.y + scr.h * 0.28,
        None,
        shade_rgba(0xF2, 0xF5, 0xF6, 1.0, b),
    );

    let btn_y = scr.y + scr.h * 0.62;
    let btn_h = (scr.h * 0.255).max(15.0);
    let gap = 5.0;
    let pad = 6.0;
    let bw = ((scr.w - pad * 2.0 - gap) * 0.5).max(16.0);
    let btn_pt = (btn_h * 0.36).clamp(7.5, 10.5);

    let opts: [(HitKey, &str, (u8, u8, u8)); 2] = [
        (HitKey::ExitStay, "Ở lại", (0x8A, 0x96, 0x9F)),
        (HitKey::ExitYes, "Rời đi", (0xB9, 0xA0, 0xA3)),
    ];

    let txt = shade_rgba(0x20, 0x29, 0x32, 1.0, b);
    for (i, (key, label, rgb)) in opts.into_iter().enumerate() {
        let rect = Rect {
            x: scr.x + pad + i as f64 * (bw + gap),
            y: btn_y,
            w: bw,
            h: btn_h,
        };
        state.hit_rects.push((key, rect));
        draw_simple_button(cr, rect, label, rgb, (0x53, 0x61, 0x6C), txt, btn_pt, 4.0, b);
    }
}

fn draw_goodbye_screen(cr: &Context, scr: Rect, b: f64) {
    let cx = scr.x + scr.w * 0.5;
    let cy = scr.y + scr.h * 0.5;
    let (sw, sh) = (scr.w, scr.h);

    draw_face_blush(cr, cx, cy, sw, sh, rgba8(170, 181, 188, 120));

    let fg = shade_rgba(0xF2, 0xF5, 0xF6, 1.0, b);
    let eye_r = sw.min(sh) * 0.13;
    cr.set_line_join(LineJoin::Round);
    draw_happy_eyes(cr, cx, cy - sh * 0.06, sw * 0.22, eye_r, fg, (sw * 0.025).max(2.0));

    set_color(cr, fg);
    cr.set_line_width((sw * 0.024).max(1.8));
    draw_smile(cr, cx, cy + sh * 0.19, sw, sh);
}

fn draw_goodnight_screen(cr: &Context, scr: Rect, state: &AppState) {
    let b = state.brightness_factor;
    let cx = scr.x + scr.w * 0.5;
    let top_y = scr.y + scr.h * 0.22;
    let sw = scr.w;

    // Má hồng
    let cheek = rgba8(255, 118, 138, 135);
    for bx in [cx - sw * 0.32 + sw * 0.07, cx + sw * 0.18 + sw * 0.07] {
        draw_ellipse(
            cr,
            bx,
            top_y + 4.0 + scr.h * 0.04,
            sw * 0.07,
            scr.h * 0.04,
            cheek,
        );
    }

    // Mắt nhắm
    let fg = shade_rgba(0xE6, 0xF5, 0xFA, 1.0, b);
    draw_happy_eyes(cr, cx, top_y, sw * 0.20, sw * 0.10, fg, 2.0);

    // Hai ngôi sao trang trí ở góc
    let star_col = rgba8(180, 231, 243, 255);
    let star_font = sans((scr.h * 0.11).max(8.0));
    draw_text_centered(cr, "✦", &star_font, scr.x + 12.0, scr.y + 12.0, None, star_col);
    draw_text_centered(
        cr,
        "✧",
        &star_font,
        scr.x + scr.w - 14.0,
        scr.y + 13.0,
        None,
        star_col,
    );

    // Lời chúc hiện dần từng dòng
    let count = (state.goodnight_step as usize).min(GOODNIGHT_LINES.len());
    if count > 0 {
        let shown = GOODNIGHT_LINES[..count].join("\n");
        let pt = (scr.h * 0.075).clamp(6.5, 9.5);
        draw_text_centered(
            cr,
            &shown,
            &sans_bold(pt),
            cx,
            scr.y + scr.h * 0.72,
            Some((scr.w - 14.0).max(20.0)),
            fg,
        );
    }
}

fn draw_speech_bubble(cr: &Context, text: &str, cx: f64, bottom_y: f64, max_w: f64) {
    if text.is_empty() {
        return;
    }
    let ink = rgba8(52, 65, 76, 255);
    let paper = rgba8(255, 255, 255, 245);
    let pad_x = 10.0;
    let pad_y = 6.0;
    let avail_w = (max_w - 10.0 - pad_x * 2.0).max(60.0);

    // Thu nhỏ chữ cho vừa một dòng nếu có thể.
    let mut pt = (max_w * 0.043).clamp(8.0, 12.5);
    let (mut tw, _) = measure_text(cr, text, &sans(pt), None);
    while tw > avail_w && pt > 7.0 {
        pt -= 0.5;
        (tw, _) = measure_text(cr, text, &sans(pt), None);
    }
    let font = sans(pt);

    let one_line = tw <= avail_w;
    let box_w = if one_line {
        (tw + pad_x * 2.0).max(90.0).min((max_w - 10.0).max(1.0))
    } else {
        (max_w - 10.0).max(90.0)
    };
    // Chữ có sẵn '\n' vẫn hiển thị nhiều dòng; chỉ ép wrap khi quá rộng.
    let wrap_w = if one_line { None } else { Some(box_w - pad_x * 2.0) };
    let (_, th) = measure_text(cr, text, &font, wrap_w);
    let box_h = (th + pad_y * 2.0).max(24.0);

    let bx = (cx - box_w * 0.5).clamp(5.0, (max_w - box_w - 5.0).max(5.0));
    let by = (bottom_y - box_h - 6.0).max(4.0);

    draw_rounded_rect(cr, bx, by, box_w, box_h, 9.0);
    fill_stroke(cr, paper, ink, 1.6);

    // Đuôi bong bóng
    let tail_x = cx.max(bx + 12.0).min(bx + box_w - 12.0);
    cr.new_path();
    cr.move_to(tail_x - 5.0, by + box_h - 0.5);
    cr.line_to(tail_x + 5.0, by + box_h - 0.5);
    cr.line_to(tail_x, by + box_h + 6.0);
    cr.close_path();
    fill_color(cr, paper);

    cr.new_path();
    cr.move_to(tail_x - 5.0, by + box_h);
    cr.line_to(tail_x, by + box_h + 6.0);
    cr.line_to(tail_x + 5.0, by + box_h);
    stroke_color(cr, ink, 1.6);

    draw_text_centered(
        cr,
        text,
        &font,
        bx + box_w * 0.5,
        by + box_h * 0.5,
        wrap_w,
        ink,
    );
}

// ───────────────────────────── Vẽ robot ─────────────────────────────

const SHELL_LIGHT: Rgba = (0.94117647, 0.94117647, 0.9254902, 1.0);

/// Bảng màu của robot, đã nhân theo độ sáng hiện tại.
struct Palette {
    b: f64,
    outline: Rgba,
    joint: Rgba,
    screen_bg: Rgba,
    rivet: Rgba,
    rivet_stroke: Rgba,
    etch: Rgba,
}

impl Palette {
    fn new(b: f64) -> Self {
        Self {
            b,
            outline: shade_rgba(0x34, 0x41, 0x4C, 1.0, b),
            joint: shade_rgba(0x8A, 0x96, 0x9F, 1.0, b),
            screen_bg: shade_rgba(0x20, 0x29, 0x32, 1.0, b),
            rivet: shade_rgba(0x68, 0x77, 0x82, 1.0, b),
            rivet_stroke: shade_rgba(0x35, 0x46, 0x53, 1.0, b),
            etch: shade_rgba(0x53, 0x61, 0x6C, 1.0, b),
        }
    }
}

/// Kích thước và vị trí các bộ phận (tính một lần cho mỗi khung hình).
#[derive(Clone, Copy)]
struct Geo {
    cx: f64,
    ant_h: f64,
    head_x: f64,
    head_y: f64,
    head_w: f64,
    head_h: f64,
    neck_y: f64,
    neck_h: f64,
    body_x: f64,
    body_y: f64,
    body_w: f64,
    body_h: f64,
    leg_y: f64,
    leg_h: f64,
    stroke_w: f64,
}

impl Geo {
    fn new(width: f64, height: f64, state: &AppState) -> Self {
        // Tổng chiều cao các bộ phận (ăng-ten + đầu + cổ + thân + chân) tính theo bot_h.
        const BODY_UNITS: f64 = 0.13 + 0.44 + 0.035 + 0.29 * 1.10 + 0.105 * 1.05;
        // Chừa chỗ phía trên cho bong bóng thoại, và phía dưới cho nhịp lắc + nét viền
        // để bàn chân không bị cắt ở mép cửa sổ.
        let top_margin = height * 0.21;
        let bottom_pad = 8.0;
        let bot_h = ((height - top_margin - bottom_pad) / BODY_UNITS).max(90.0);
        let cx = width * 0.5;

        let ant_h = bot_h * 0.13;
        let head_w = bot_h * 0.56;
        let head_h = bot_h * 0.44;
        let neck_h = bot_h * 0.035;
        let body_w = bot_h * (0.46 * 1.08);
        let body_h = bot_h * (0.29 * 1.10);
        let leg_h = bot_h * (0.105 * 1.05);

        let head_y = top_margin + ant_h + state.head_bob;
        let neck_y = head_y + head_h;
        let body_y = neck_y + neck_h + state.head_bob * 0.4;

        Self {
            cx,
            ant_h,
            head_x: cx - head_w * 0.5,
            head_y,
            head_w,
            head_h,
            neck_y,
            neck_h,
            body_x: cx - body_w * 0.5,
            body_y,
            body_w,
            body_h,
            leg_y: body_y + body_h + state.foot_bob,
            leg_h,
            stroke_w: (bot_h * 0.011).max(1.8),
        }
    }
}

fn draw_robot(cr: &Context, width: f64, height: f64, state: &mut AppState) {
    // Nền trong suốt
    cr.set_operator(Operator::Clear);
    let _ = cr.paint();
    cr.set_operator(Operator::Over);

    state.hit_rects.clear();

    let p = Palette::new(state.brightness_factor);
    let g = Geo::new(width, height, state);

    cr.set_line_join(LineJoin::Round);
    cr.set_line_cap(LineCap::Round);

    draw_antennas(cr, &g, &p, state);
    draw_legs(cr, &g, &p);
    draw_arms(cr, &g, &p, state);
    draw_neck_and_torso(cr, &g, &p, state);
    draw_head_and_screen(cr, &g, &p, state);

    // Bong bóng thoại phía trên đầu
    if !state.speech_text.is_empty()
        && (state.is_on
            || matches!(
                state.screen_mode,
                ScreenMode::ExitConfirm | ScreenMode::Goodbye
            ))
    {
        draw_speech_bubble(
            cr,
            &state.speech_text,
            g.cx,
            g.head_y - g.ant_h * 0.55,
            width,
        );
    }
}

fn draw_antennas(cr: &Context, g: &Geo, p: &Palette, state: &AppState) {
    let droop = !state.is_on
        || matches!(
            state.emotion,
            Emotion::Sad | Emotion::Pout | Emotion::Sleepy
        );
    let bulb = if state.is_on {
        shade_rgba(0xF0, 0xF4, 0xF7, 1.0, p.b)
    } else {
        shade_rgba(0x77, 0x7D, 0x83, 1.0, p.b)
    };

    for side in [-1.0, 1.0] {
        let _ = cr.save();
        cr.translate(g.cx + side * g.head_w * 0.16, g.head_y + 2.0);
        let angle_deg = if droop {
            side * 34.0
        } else {
            side * (16.0 + state.antenna_angle)
        };
        cr.rotate(angle_deg.to_radians());

        let tip = (side * g.ant_h * 0.12, -g.ant_h);
        cr.new_path();
        cr.move_to(0.0, 0.0);
        quad_to(
            cr,
            (0.0, 0.0),
            (side * g.ant_h * 0.35, -g.ant_h * 0.55),
            tip,
        );
        stroke_color(cr, p.outline, (g.ant_h * 0.12).max(2.2));

        cr.new_sub_path();
        cr.arc(tip.0, tip.1, (g.ant_h * 0.16).max(3.0), 0.0, TAU);
        fill_stroke(cr, bulb, p.outline, g.stroke_w);
        let _ = cr.restore();
    }
}

fn draw_legs(cr: &Context, g: &Geo, p: &Palette) {
    let leg_w = g.body_w * 0.22;
    for side in [-1.0, 1.0] {
        let lx = g.cx + side * g.body_w * 0.22 - leg_w * 0.5;

        draw_rounded_rect(
            cr,
            lx + leg_w * 0.15,
            g.leg_y - 2.0,
            leg_w * 0.7,
            g.leg_h * 0.45,
            3.0,
        );
        fill_stroke(cr, p.joint, p.outline, g.stroke_w);

        draw_rounded_rect(
            cr,
            lx - 2.0,
            g.leg_y + g.leg_h * 0.35,
            leg_w + 4.0,
            g.leg_h * 0.65,
            5.0,
        );
        fill_stroke(cr, SHELL_LIGHT, p.outline, g.stroke_w);
    }
}

#[derive(Clone, Copy)]
struct ArmDims {
    w: f64,
    h: f64,
    rivet_r: f64,
    dot_r: f64,
}

/// Vỏ cánh tay (hệ toạ độ đã tịnh tiến về khớp vai) và chiếc đinh tán ở góc.
fn draw_arm_shell(cr: &Context, d: ArmDims, x0: f64, rivet_x: f64, p: &Palette, stroke_w: f64) {
    draw_rounded_rect(cr, x0, -6.0, d.w, d.h, 6.0);
    fill_stroke(cr, SHELL_LIGHT, p.outline, stroke_w);
    draw_rivet(
        cr,
        rivet_x,
        -2.0 + d.rivet_r,
        d.rivet_r,
        p.rivet,
        p.rivet_stroke,
        1.2,
    );
}

/// Vùng bấm của cánh tay sau khi xoay, nới thêm 2 px mỗi phía.
fn arm_hit_rect(pivot_x: f64, pivot_y: f64, x0: f64, d: ArmDims, angle_deg: f64) -> Rect {
    let r = rotated_bounds(
        pivot_x,
        pivot_y,
        [
            (x0, -6.0),
            (x0 + d.w, -6.0),
            (x0, -6.0 + d.h),
            (x0 + d.w, -6.0 + d.h),
        ],
        angle_deg,
    );
    Rect {
        x: r.x - 2.0,
        y: r.y - 2.0,
        w: r.w + 4.0,
        h: r.h + 4.0,
    }
}

/// Bánh răng ghép từ đinh tán (cùng kiểu với chữ "E" bên tay trái):
/// 8 răng nhô ra xen kẽ 8 đinh ở rãnh, chính giữa là một đinh làm trục.
fn draw_gear(cr: &Context, cx: f64, cy: f64, r_out: f64, dot_r: f64, rotation: f64, p: &Palette) {
    const TEETH: usize = 8;
    let r_valley = r_out * 0.74;
    let step = TAU / (TEETH * 2) as f64;

    for i in 0..TEETH * 2 {
        let a = rotation + i as f64 * step - PI * 0.5;
        // Chẵn: đỉnh răng (nhô ra), lẻ: rãnh giữa hai răng (lùi vào).
        let r = if i % 2 == 0 { r_out } else { r_valley };
        draw_rivet(
            cr,
            cx + r * a.cos(),
            cy + r * a.sin(),
            dot_r,
            p.rivet,
            p.rivet_stroke,
            0.8,
        );
    }
    draw_rivet(cr, cx, cy, dot_r * 1.15, p.rivet, p.rivet_stroke, 0.8);
}

fn draw_arms(cr: &Context, g: &Geo, p: &Palette, state: &mut AppState) {
    let w = g.body_w * 0.22;
    let d = ArmDims {
        w,
        h: g.body_h * 0.66,
        rivet_r: (w * 0.067).max(1.6),
        dot_r: (w * 0.048).max(1.2),
    };
    let pivot_y = g.body_y + g.body_h * 0.14 + 6.0;

    // ── Tay trái: lưới đinh tán hình chữ "E" — bấm để thoát ──
    let left_pivot_x = g.body_x - 2.0;
    let left_rot = state.arm_angle + state.left_arm_raise;
    let _ = cr.save();
    cr.translate(left_pivot_x, pivot_y);
    cr.rotate(left_rot.to_radians());
    draw_arm_shell(cr, d, -d.w, -d.rivet_r - 4.0, p, g.stroke_w);

    let _ = cr.save();
    cr.translate(-d.w * 0.5, -6.0 + d.h * 0.53);
    cr.rotate((-12.0_f64).to_radians());
    let step_x = d.w * 0.155;
    let step_y = d.w * 0.145;
    const E_DOTS: [(f64, f64); 10] = [
        (-1.0, -2.0),
        (-1.0, -1.0),
        (-1.0, 0.0),
        (-1.0, 1.0),
        (-1.0, 2.0),
        (0.0, -2.0),
        (1.0, -2.0),
        (0.0, 0.0),
        (0.0, 2.0),
        (1.0, 2.0),
    ];
    for (gx, gy) in E_DOTS {
        draw_rivet(
            cr,
            gx * step_x,
            gy * step_y,
            d.dot_r,
            p.rivet,
            p.rivet_stroke,
            0.8,
        );
    }
    let _ = cr.restore();
    let _ = cr.restore();
    state.hit_rects.push((
        HitKey::Exit,
        arm_hit_rect(left_pivot_x, pivot_y, -d.w, d, left_rot),
    ));

    // ── Tay phải: bánh răng đinh tán — bấm để mở cài đặt ──
    let right_pivot_x = g.body_x + g.body_w + 2.0;
    let right_rot = if state.micro == MicroBehavior::Thinking && state.is_on {
        -65.0
    } else {
        -state.arm_angle
    };
    let _ = cr.save();
    cr.translate(right_pivot_x, pivot_y);
    cr.rotate(right_rot.to_radians());
    draw_arm_shell(cr, d, 0.0, 4.0 + d.rivet_r, p, g.stroke_w);

    // Biểu tượng "Cài đặt": bánh răng bằng đinh tán, quay khi rê chuột vào.
    let hovered = state.hover_key == Some(HitKey::Settings);
    let spin = if hovered { state.t_phase * 0.6 } else { 0.0 };
    draw_gear(
        cr,
        d.w * 0.5,
        -6.0 + d.h * 0.53,
        d.w * 0.36,
        d.dot_r,
        spin,
        p,
    );
    let _ = cr.restore();
    state.hit_rects.push((
        HitKey::Settings,
        arm_hit_rect(right_pivot_x, pivot_y, 0.0, d, right_rot),
    ));
}

fn draw_panel_button(
    cr: &Context,
    r: Rect,
    label: &str,
    enabled: bool,
    hovered: bool,
    b: f64,
    font_pt: f64,
) {
    let face_scale = if !enabled {
        b * 0.65
    } else if hovered {
        b * 1.12
    } else {
        b
    };
    let rim = shade_rgba(0x13, 0x1C, 0x24, 1.0, b);
    let bezel = shade_rgba(0x4D, 0x5A, 0x64, 1.0, b);
    let face = shade_rgba(0x8A, 0x96, 0x9F, 1.0, face_scale);
    let hi = shade_rgba(0x66, 0x72, 0x7B, 1.0, if enabled { b } else { b * 0.65 });
    let txt = shade_rgba(0x20, 0x29, 0x32, 1.0, if enabled { b } else { b * 0.7 });

    draw_rounded_rect(cr, r.x, r.y, r.w, r.h, 5.0);
    fill_stroke(cr, bezel, rim, 1.4);

    draw_rounded_rect(
        cr,
        r.x + 2.0,
        r.y + 2.0,
        (r.w - 4.0).max(4.0),
        (r.h - 4.0).max(4.0),
        3.5,
    );
    fill_color(cr, face);

    cr.move_to(r.x + 5.0, r.y + 3.0);
    cr.line_to((r.x + r.w - 5.0).max(r.x + 5.0), r.y + 3.0);
    stroke_color(cr, hi, 1.0);

    draw_text_centered(
        cr,
        label,
        &sans_bold(font_pt),
        r.x + r.w * 0.5,
        r.y + r.h * 0.5,
        None,
        txt,
    );
}

/// Cổ, thân và bảng 2×2 nút điều khiển (On / Off / Emo / Clock).
fn draw_neck_and_torso(cr: &Context, g: &Geo, p: &Palette, state: &mut AppState) {
    // Cổ
    draw_rounded_rect(
        cr,
        g.cx - g.body_w * 0.16,
        g.neck_y - 2.0,
        g.body_w * 0.32,
        g.neck_h + 4.0,
        3.0,
    );
    fill_stroke(cr, p.joint, p.outline, g.stroke_w);

    // Thân
    draw_rounded_rect(
        cr,
        g.body_x + 3.0,
        g.body_y + 2.0,
        g.body_w - 6.0,
        g.body_h - 4.0,
        10.0,
    );
    fill_stroke(cr, SHELL_LIGHT, p.outline, g.stroke_w);

    // Bảng điều khiển
    let pad = g.body_w * 0.11;
    let panel_x = g.body_x + pad;
    let panel_y = g.body_y + pad * 0.85;
    let panel_w = g.body_w - pad * 2.0;
    let panel_h = g.body_h - pad * 1.7;

    draw_rounded_rect(cr, panel_x, panel_y, panel_w, panel_h, 7.0);
    fill_stroke(cr, p.screen_bg, p.outline, g.stroke_w);

    let gap = 4.0;
    let inner_pad = 5.0;
    let bw = ((panel_w - inner_pad * 2.0 - gap) * 0.5).max(14.0);
    let bh = ((panel_h - inner_pad * 2.0 - gap) * 0.5).max(12.0);
    let font_pt = (bh * 0.34).clamp(7.0, 11.5);

    // Các nút điều khiển bị khoá khi đang hỏi thoát / tạm biệt / chuẩn bị ngủ.
    let usable = state.is_on && !state.input_locked();
    let buttons = [
        (HitKey::On, "On", 0.0, 0.0, state.on_allowed()),
        (HitKey::Off, "Off", 1.0, 0.0, usable),
        (HitKey::Emotion, "Emo", 0.0, 1.0, usable),
        (HitKey::Clock, "Clock", 1.0, 1.0, usable),
    ];

    for (key, label, col, row, enabled) in buttons {
        let r = Rect {
            x: panel_x + inner_pad + col * (bw + gap),
            y: panel_y + inner_pad + row * (bh + gap),
            w: bw,
            h: bh,
        };
        state.hit_rects.push((key, r));
        let hovered = enabled && state.hover_key == Some(key);
        draw_panel_button(cr, r, label, enabled, hovered, p.b, font_pt);
    }
}

/// Đầu, đinh tán, khung màn hình và nội dung màn hình theo `screen_mode`.
fn draw_head_and_screen(cr: &Context, g: &Geo, p: &Palette, state: &mut AppState) {
    draw_rounded_rect(cr, g.head_x, g.head_y, g.head_w, g.head_h, 14.0);
    fill_stroke(cr, SHELL_LIGHT, p.outline, g.stroke_w);

    let rivet_r = (g.head_w * 0.022 * 0.85).max(1.7);
    for rx in [g.head_x + 7.0, g.head_x + g.head_w - 7.0] {
        for ry in [g.head_y + 7.0, g.head_y + g.head_h - 7.0] {
            cr.new_sub_path();
            cr.arc(rx, ry, rivet_r, 0.0, TAU);
            fill_color(cr, p.rivet);
        }
    }

    let margin = g.head_w * 0.10;
    let scr = Rect {
        x: g.head_x + margin,
        y: g.head_y + margin * 0.9,
        w: (g.head_w - margin * 2.0).max(30.0),
        h: (g.head_h - margin * 1.8).max(24.0),
    };

    draw_rounded_rect(cr, scr.x, scr.y, scr.w, scr.h, 9.0);
    fill_stroke(cr, p.screen_bg, p.etch, 1.6);

    match state.screen_mode {
        ScreenMode::Clock => draw_clock_screen(cr, scr, p.b),
        ScreenMode::Stopwatch => draw_stopwatch_screen(cr, scr, state),
        ScreenMode::ExitConfirm => draw_exit_confirm_screen(cr, scr, state),
        ScreenMode::Goodbye => draw_goodbye_screen(cr, scr, p.b),
        ScreenMode::Goodnight => draw_goodnight_screen(cr, scr, state),
        ScreenMode::Face => draw_face_expression(cr, scr, state),
    }
}

// ───────────────────────────── Hộp thoại cài đặt ─────────────────────────────

fn open_settings_dialog(
    parent: &ApplicationWindow,
    area: &DrawingArea,
    state: Rc<RefCell<AppState>>,
) {
    let current_cfg = state.borrow().config.clone();

    let dialog = gtk4::Window::builder()
        .title("Cài đặt Kai")
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .default_width(360)
        .build();

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    vbox.set_margin_top(16);
    vbox.set_margin_bottom(16);
    vbox.set_margin_start(16);
    vbox.set_margin_end(16);

    let owner_lbl = gtk4::Label::new(Some("Thông tin chủ nhân:"));
    owner_lbl.set_halign(gtk4::Align::Start);
    let owner_entry = gtk4::Entry::new();
    owner_entry.set_max_length(MAX_OWNER_CHARS as i32);
    owner_entry.set_text(&current_cfg.owner_name);

    let height_lbl = gtk4::Label::new(Some(&format!(
        "Kích thước Kai ({MIN_HEIGHT:.0} - {MAX_HEIGHT:.0} px):"
    )));
    height_lbl.set_halign(gtk4::Align::Start);
    let height_adj =
        gtk4::Adjustment::new(current_cfg.height, MIN_HEIGHT, MAX_HEIGHT, 5.0, 20.0, 0.0);
    let height_spin = gtk4::SpinButton::new(Some(&height_adj), 5.0, 0);

    let auto_check = gtk4::CheckButton::with_label("Bật tự động đổi biểu cảm");
    auto_check.set_active(current_cfg.auto_emotion);

    let interval_lbl = gtk4::Label::new(Some(&format!(
        "Thời gian đổi biểu cảm (phút, {MIN_EMOTION_MINUTES} - {MAX_EMOTION_MINUTES}):"
    )));
    interval_lbl.set_halign(gtk4::Align::Start);
    let interval_adj = gtk4::Adjustment::new(
        current_cfg.auto_emotion_minutes as f64,
        MIN_EMOTION_MINUTES as f64,
        MAX_EMOTION_MINUTES as f64,
        1.0,
        5.0,
        0.0,
    );
    let interval_spin = gtk4::SpinButton::new(Some(&interval_adj), 1.0, 0);

    let ver_lbl = gtk4::Label::new(Some(&format!(
        "Phiên bản: Kai {}",
        env!("CARGO_PKG_VERSION")
    )));
    ver_lbl.set_halign(gtk4::Align::Center);

    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk4::Align::End);
    let cancel_btn = gtk4::Button::with_label("Hủy");
    let save_btn = gtk4::Button::with_label("Lưu");
    btn_box.append(&cancel_btn);
    btn_box.append(&save_btn);

    vbox.append(&owner_lbl);
    vbox.append(&owner_entry);
    vbox.append(&height_lbl);
    vbox.append(&height_spin);
    vbox.append(&auto_check);
    vbox.append(&interval_lbl);
    vbox.append(&interval_spin);
    vbox.append(&ver_lbl);
    vbox.append(&btn_box);
    dialog.set_child(Some(&vbox));

    let dlg_cancel = dialog.clone();
    cancel_btn.connect_clicked(move |_| dlg_cancel.close());

    // Esc để đóng hộp thoại
    let esc = EventControllerKey::new();
    let dlg_esc = dialog.clone();
    esc.connect_key_pressed(move |_, key, _, _| {
        if key == gtk4::gdk::Key::Escape {
            dlg_esc.close();
            Propagation::Stop
        } else {
            Propagation::Proceed
        }
    });
    dialog.add_controller(esc);

    let dlg_save = dialog.clone();
    let parent_win = parent.clone();
    let area_save = area.clone();
    save_btn.connect_clicked(move |_| {
        let (w, h) = {
            let mut st = state.borrow_mut();
            st.config = Config {
                owner_name: owner_entry.text().to_string(),
                height: height_spin.value(),
                auto_emotion: auto_check.is_active(),
                auto_emotion_minutes: interval_spin.value().round().max(0.0) as u32,
                ..st.config.clone()
            }
            .normalized();
            save_config(&st.config);
            st.restart_auto_emotion_timer();
            if st.is_on {
                st.reset_idle_timer();
            }
            window_size(st.config.height)
        };

        // Phải đổi cả kích thước tối thiểu của vùng vẽ, nếu không cửa sổ chỉ to ra được.
        area_save.set_size_request(w, h);
        parent_win.set_default_size(w, h);
        parent_win.set_size_request(w, h);
        area_save.queue_draw();
        dlg_save.close();
    });

    dialog.present();
}

// ───────────────────────────── Giao diện chính ─────────────────────────────

fn set_area_cursor(area: &DrawingArea, name: Option<&str>) {
    let cursor = name.and_then(|n| gtk4::gdk::Cursor::from_name(n, None));
    area.set_cursor(cursor.as_ref());
}

/// Bắt đầu kéo cả cửa sổ (cửa sổ không viền nên phải tự xử lý).
fn start_window_drag(win: &ApplicationWindow, gesture: &GestureClick, x: f64, y: f64) {
    let Some(surface) = win.surface() else {
        return;
    };
    let Ok(toplevel) = surface.downcast::<gtk4::gdk::Toplevel>() else {
        return;
    };
    if let Some(device) = gesture.device() {
        toplevel.begin_move(&device, 1, x, y, gesture.current_event_time());
    }
}

fn build_ui(app: &Application) {
    // Mở Kai lần thứ hai: đưa cửa sổ cũ lên thay vì tạo thêm một Kai nữa.
    if let Some(win) = app.active_window() {
        win.present();
        return;
    }

    let state = Rc::new(RefCell::new(AppState::new()));
    let (init_w, init_h) = window_size(state.borrow().config.height);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Kai")
        .default_width(init_w)
        .default_height(init_h)
        .resizable(false)
        .decorated(false)
        .build();
    window.set_size_request(init_w, init_h);
    // Chỉ cửa sổ Kai trong suốt; hộp thoại cài đặt giữ nền bình thường.
    window.add_css_class("kai-window");

    let css_provider = gtk4::CssProvider::new();
    css_provider.load_from_data(
        "window.kai-window, window.kai-window.background { \
            background-color: transparent; background-image: none; box-shadow: none; }",
    );
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &css_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let drawing_area = DrawingArea::new();
    drawing_area.set_hexpand(true);
    drawing_area.set_vexpand(true);
    drawing_area.set_size_request(init_w, init_h);
    window.set_child(Some(&drawing_area));

    let st_draw = state.clone();
    drawing_area.set_draw_func(move |_area, cr, w, h| {
        let mut st = st_draw.borrow_mut();
        draw_robot(cr, w.max(1) as f64, h.max(1) as f64, &mut st);
    });

    // ── Di chuột: đổi con trỏ + làm sáng nút ──
    let motion = EventControllerMotion::new();

    let area_enter = drawing_area.clone();
    motion.connect_enter(move |_, _, _| {
        set_area_cursor(&area_enter, Some("grab"));
    });

    let st_motion = state.clone();
    let area_motion = drawing_area.clone();
    motion.connect_motion(move |_, x, y| {
        let mut st = st_motion.borrow_mut();
        let found = st
            .hit_rects
            .iter()
            .rev()
            .find(|(_, r)| r.contains(x, y))
            .map(|(k, _)| *k);
        if found != st.hover_key {
            st.hover_key = found;
            set_area_cursor(
                &area_motion,
                Some(if found.is_some() { "pointer" } else { "grab" }),
            );
            area_motion.queue_draw();
        }
    });

    let st_leave = state.clone();
    let area_leave = drawing_area.clone();
    motion.connect_leave(move |_| {
        if st_leave.borrow_mut().hover_key.take().is_some() {
            area_leave.queue_draw();
        }
        set_area_cursor(&area_leave, None);
    });
    drawing_area.add_controller(motion);

    // ── Bấm chuột trái ──
    let click = GestureClick::new();
    click.set_button(1);
    let st_click = state.clone();
    let area_click = drawing_area.clone();
    let win_click = window.clone();
    click.connect_pressed(move |gesture, n_press, x, y| {
        if n_press != 1 {
            return;
        }

        let mut open_settings = false;
        let hit = {
            let mut st = st_click.borrow_mut();
            let hit = st
                .hit_rects
                .iter()
                .rev()
                .find(|(_, r)| r.contains(x, y))
                .map(|(k, _)| *k);

            match hit {
                Some(HitKey::On) => st.action_on(),
                Some(HitKey::Off) => st.action_off(),
                Some(HitKey::Emotion) => st.action_emotion(),
                Some(HitKey::Clock) => st.action_clock(),
                Some(HitKey::Exit) => st.action_exit_prompt(),
                Some(HitKey::ExitStay) => st.action_exit_stay(),
                Some(HitKey::ExitYes) => st.action_exit_confirm(),
                Some(HitKey::SwStart) => {
                    st.sw_start();
                    st.reset_idle_timer();
                }
                Some(HitKey::SwStop) => {
                    st.sw_pause();
                    st.reset_idle_timer();
                }
                Some(HitKey::SwReset) => {
                    st.sw_reset();
                    st.reset_idle_timer();
                }
                Some(HitKey::Settings) => open_settings = st.settings_allowed(),
                None => {
                    // Kéo cửa sổ cũng tính là tương tác.
                    if st.is_on {
                        st.reset_idle_timer();
                    }
                }
            }
            hit
        };

        // Mở dialog sau khi đã thả borrow của state.
        if open_settings {
            open_settings_dialog(&win_click, &area_click, st_click.clone());
        }
        area_click.queue_draw();

        if hit.is_none() {
            start_window_drag(&win_click, gesture, x, y);
        }
    });
    drawing_area.add_controller(click);

    // ── Phím Esc: huỷ hỏi thoát ──
    let key_ctrl = EventControllerKey::new();
    let st_key = state.clone();
    let area_key = drawing_area.clone();
    key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gtk4::gdk::Key::Escape {
            let mut st = st_key.borrow_mut();
            if st.screen_mode == ScreenMode::ExitConfirm {
                st.action_exit_stay();
                drop(st);
                area_key.queue_draw();
                return Propagation::Stop;
            }
        }
        Propagation::Proceed
    });
    window.add_controller(key_ctrl);

    // ── Vòng lặp cập nhật ──
    let st_tick = state.clone();
    let area_tick = drawing_area.clone();
    let app_tick = app.clone();
    glib::timeout_add_local(Duration::from_millis(TICK_MS), move || {
        let outcome = st_tick
            .borrow_mut()
            .tick(Instant::now(), Local::now().hour());

        if outcome.quit {
            app_tick.quit();
            return ControlFlow::Break;
        }
        if outcome.redraw {
            area_tick.queue_draw();
        }
        ControlFlow::Continue
    });

    window.present();
}

fn main() -> glib::ExitCode {
    let app = Application::builder()
        .application_id("vn.dnn.Kai")
        .build();
    app.connect_activate(build_ui);
    app.run()
}
