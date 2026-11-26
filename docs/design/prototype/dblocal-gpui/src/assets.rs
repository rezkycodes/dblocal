//! Asset source: the bundled gpui-kit icons plus the extra Lucide icons DBLocal uses.
//! Only icons registered here are embedded into the binary.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

icon_assets!(
    ExtraIcons,
    [
        Database,
        KeyRound,
        ShieldCheck,
        Package,
        Download,
        Power,
        ScrollText,
        SlidersHorizontal,
        Table2,
        TriangleAlert,
        WifiOff,
        Archive,
        HardDrive,
        Server,
        CircleCheck,
        Activity,
        Terminal
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
