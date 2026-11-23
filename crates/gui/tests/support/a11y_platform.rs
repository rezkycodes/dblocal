//! A headless GPUI platform that actually builds the accessibility tree.
//!
//! GPUI builds the a11y tree only when the platform's window invokes the
//! activation callback handed to `a11y_init`. The stock test platform leaves
//! that callback on the floor (its `a11y_init` is the default no-op), so none
//! of the existing GUI tests ever reach the `Duplicate a11y node id` assertion
//! in `gpui::window::a11y`. This platform delegates everything to the real
//! Linux headless platform and only wraps the window: it calls the activation
//! callback as soon as the window exists, so every later frame builds the tree.
//!
//! Test-only: no display server, no compositor, no GPU.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui_kit::accesskit::TreeUpdate;
use gpui_kit::{
    A11yCallbacks, Action, ActivityGuard, AnyWindowHandle, BackgroundExecutor, Bounds, Capslock,
    ClipboardItem, CursorStyle, DispatchEventResult, ForegroundExecutor, GpuSpecs, Keymap, Menu,
    MenuItem, Modifiers, PathPromptOptions, Pixels, Platform, PlatformAtlas, PlatformDisplay,
    PlatformInfo, PlatformInput, PlatformInputHandler, PlatformKeyboardLayout,
    PlatformKeyboardMapper, PlatformTextSystem, PlatformWindow, Point, PromptButton, PromptLevel,
    RequestFrameOptions, Scene, Size, Task, ThermalState, WindowAppearance,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowParams, WindowVisibility,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

type Oneshot<T> = futures::channel::oneshot::Receiver<T>;

/// A platform that forwards to the headless one and forces a11y on.
pub struct A11yPlatform {
    inner: Rc<dyn Platform>,
    trees: Arc<Mutex<Vec<TreeUpdate>>>,
}

impl A11yPlatform {
    /// The Linux headless platform behind an a11y-activating wrapper.
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            inner: gpui_kit::platform::current_platform(true),
            trees: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Every tree GPUI sent for this platform's windows, oldest first.
    pub fn trees(&self) -> Vec<TreeUpdate> {
        self.trees.lock().map(|t| t.clone()).unwrap_or_default()
    }
}

impl gpui_kit::Platform for A11yPlatform {
    fn platform_info(&self) -> PlatformInfo {
        self.inner.platform_info()
    }
    fn background_executor(&self) -> BackgroundExecutor {
        self.inner.background_executor()
    }
    fn foreground_executor(&self) -> ForegroundExecutor {
        self.inner.foreground_executor()
    }
    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.inner.text_system()
    }
    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        // Do not block a test on a platform event loop: run the launch callback
        // and return, so the caller drives frames itself via run_embedded.
        on_finish_launching();
    }
    fn quit(&self) {
        self.inner.quit();
    }
    fn restart(&self, binary_path: Option<PathBuf>, arguments: Vec<std::ffi::OsString>) {
        self.inner.restart(binary_path, arguments);
    }
    fn activate(&self, ignoring_other_apps: bool) {
        self.inner.activate(ignoring_other_apps);
    }
    fn hide(&self) {
        self.inner.hide();
    }
    fn hide_other_apps(&self) {
        self.inner.hide_other_apps();
    }
    fn unhide_other_apps(&self) {
        self.inner.unhide_other_apps();
    }
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.inner.displays()
    }
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.inner.primary_display()
    }
    fn on_displays_changed(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_displays_changed(callback);
    }
    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.inner.active_window()
    }
    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> anyhow::Result<Box<dyn PlatformWindow>> {
        let inner = self.inner.open_window(handle, options)?;
        Ok(Box::new(A11yWindow {
            inner,
            trees: self.trees.clone(),
        }))
    }
    fn window_appearance(&self) -> WindowAppearance {
        self.inner.window_appearance()
    }
    fn open_url(&self, url: &str) {
        self.inner.open_url(url);
    }
    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.inner.on_open_urls(callback);
    }
    fn register_url_scheme(&self, url: &str) -> Task<anyhow::Result<()>> {
        self.inner.register_url_scheme(url)
    }
    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> Oneshot<anyhow::Result<Option<Vec<PathBuf>>>> {
        self.inner.prompt_for_paths(options)
    }
    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> Oneshot<anyhow::Result<Option<PathBuf>>> {
        self.inner.prompt_for_new_path(directory, suggested_name)
    }
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        self.inner.can_select_mixed_files_and_dirs()
    }
    fn reveal_path(&self, path: &Path) {
        self.inner.reveal_path(path);
    }
    fn open_with_system(&self, path: &Path) {
        self.inner.open_with_system(path);
    }
    fn on_quit(&self, callback: Box<dyn FnMut() -> bool>) {
        self.inner.on_quit(callback);
    }
    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_reopen(callback);
    }
    fn on_system_sleep(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_system_sleep(callback);
    }
    fn on_system_wake(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_system_wake(callback);
    }
    fn set_menus(&self, menus: Vec<Menu>, keymap: &Keymap) {
        self.inner.set_menus(menus, keymap);
    }
    fn set_dock_menu(&self, menu: Vec<MenuItem>, keymap: &Keymap) {
        self.inner.set_dock_menu(menu, keymap);
    }
    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.inner.on_app_menu_action(callback);
    }
    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_will_open_app_menu(callback);
    }
    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.inner.on_validate_app_menu_command(callback);
    }
    fn thermal_state(&self) -> ThermalState {
        self.inner.thermal_state()
    }
    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_thermal_state_change(callback);
    }
    fn prevent_idle_sleep(&self, reason: &str) -> Task<anyhow::Result<ActivityGuard>> {
        self.inner.prevent_idle_sleep(reason)
    }
    fn app_path(&self) -> anyhow::Result<PathBuf> {
        self.inner.app_path()
    }
    fn path_for_auxiliary_executable(&self, name: &str) -> anyhow::Result<PathBuf> {
        self.inner.path_for_auxiliary_executable(name)
    }
    fn set_cursor_style(&self, style: CursorStyle) {
        self.inner.set_cursor_style(style);
    }
    fn hide_cursor_until_mouse_moves(&self) {
        self.inner.hide_cursor_until_mouse_moves();
    }
    fn is_cursor_visible(&self) -> bool {
        self.inner.is_cursor_visible()
    }
    fn should_auto_hide_scrollbars(&self) -> bool {
        self.inner.should_auto_hide_scrollbars()
    }
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.inner.read_from_clipboard()
    }
    fn write_to_clipboard(&self, item: ClipboardItem) {
        self.inner.write_to_clipboard(item);
    }
    fn read_from_primary(&self) -> Option<ClipboardItem> {
        self.inner.read_from_primary()
    }
    fn write_to_primary(&self, item: ClipboardItem) {
        self.inner.write_to_primary(item);
    }
    fn write_credentials(
        &self,
        url: &str,
        username: &str,
        password: &[u8],
    ) -> Task<anyhow::Result<()>> {
        self.inner.write_credentials(url, username, password)
    }
    fn read_credentials(&self, url: &str) -> Task<anyhow::Result<Option<(String, Vec<u8>)>>> {
        self.inner.read_credentials(url)
    }
    fn delete_credentials(&self, url: &str) -> Task<anyhow::Result<()>> {
        self.inner.delete_credentials(url)
    }
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        self.inner.keyboard_layout()
    }
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        self.inner.keyboard_mapper()
    }
    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_keyboard_layout_change(callback);
    }
}

/// A window that activates a11y on creation and records every tree.
struct A11yWindow {
    inner: Box<dyn PlatformWindow>,
    trees: Arc<Mutex<Vec<TreeUpdate>>>,
}

impl HasWindowHandle for A11yWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for A11yWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.inner.display_handle()
    }
}

impl gpui_kit::PlatformWindow for A11yWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.inner.bounds()
    }
    fn is_maximized(&self) -> bool {
        self.inner.is_maximized()
    }
    fn window_bounds(&self) -> WindowBounds {
        self.inner.window_bounds()
    }
    fn content_size(&self) -> Size<Pixels> {
        self.inner.content_size()
    }
    fn resize(&mut self, size: Size<Pixels>) {
        self.inner.resize(size);
    }
    fn scale_factor(&self) -> f32 {
        self.inner.scale_factor()
    }
    fn appearance(&self) -> WindowAppearance {
        self.inner.appearance()
    }
    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.inner.display()
    }
    fn mouse_position(&self) -> Point<Pixels> {
        self.inner.mouse_position()
    }
    fn modifiers(&self) -> Modifiers {
        self.inner.modifiers()
    }
    fn capslock(&self) -> Capslock {
        self.inner.capslock()
    }
    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.inner.set_input_handler(input_handler);
    }
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.inner.take_input_handler()
    }
    fn prompt(
        &self,
        level: PromptLevel,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> Option<Oneshot<usize>> {
        self.inner.prompt(level, msg, detail, answers)
    }
    fn activate(&self) {
        self.inner.activate();
    }
    fn is_active(&self) -> bool {
        self.inner.is_active()
    }
    fn visibility(&self) -> WindowVisibility {
        self.inner.visibility()
    }
    fn is_hovered(&self) -> bool {
        self.inner.is_hovered()
    }
    fn background_appearance(&self) -> WindowBackgroundAppearance {
        self.inner.background_appearance()
    }
    fn set_title(&mut self, title: &str) {
        self.inner.set_title(title);
    }
    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        self.inner.set_background_appearance(background_appearance);
    }
    fn minimize(&self) {
        self.inner.minimize();
    }
    fn zoom(&self) {
        self.inner.zoom();
    }
    fn toggle_fullscreen(&self) {
        self.inner.toggle_fullscreen();
    }
    fn is_fullscreen(&self) -> bool {
        self.inner.is_fullscreen()
    }
    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.inner.on_request_frame(callback);
    }
    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.inner.on_input(callback);
    }
    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.inner.on_active_status_change(callback);
    }
    fn on_visibility_change(&self, callback: Box<dyn FnMut(WindowVisibility)>) {
        self.inner.on_visibility_change(callback);
    }
    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.inner.on_hover_status_change(callback);
    }
    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.inner.on_resize(callback);
    }
    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_moved(callback);
    }
    fn on_display_changed(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_display_changed(callback);
    }
    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.inner.on_should_close(callback);
    }
    fn on_hit_test_window_control(&self, callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.inner.on_hit_test_window_control(callback);
    }
    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.inner.on_close(callback);
    }
    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.inner.on_appearance_changed(callback);
    }
    fn draw(&self, scene: &Scene) {
        self.inner.draw(scene);
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.inner.sprite_atlas()
    }
    fn is_subpixel_rendering_supported(&self) -> bool {
        self.inner.is_subpixel_rendering_supported()
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.inner.gpu_specs()
    }
    fn update_ime_position(&self, bounds: Bounds<Pixels>) {
        self.inner.update_ime_position(bounds);
    }
    fn a11y_init(&self, callbacks: A11yCallbacks) {
        // The stock headless window drops these callbacks, so a11y stays off.
        // Call the activation callback now (it flips gpui's active flag) and
        // hand it on, so the very next frame builds the tree.
        let activation = callbacks.activation;
        let _initial_tree = activation();
        self.inner.a11y_init(A11yCallbacks {
            activation,
            action: callbacks.action,
            deactivation: callbacks.deactivation,
        });
    }
    fn a11y_tree_update(&self, tree_update: TreeUpdate) {
        if let Ok(mut trees) = self.trees.lock() {
            trees.push(tree_update.clone());
        }
        self.inner.a11y_tree_update(tree_update);
    }
}
