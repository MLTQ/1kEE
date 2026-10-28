# settings_paths.rs

Render Settings → Files & Cache: path overrides, explicit folder/file pickers,
cache diagnostics, detailed-elevation streaming and CPU/GPU cache budgets.

Blank paths retain default/auto-detection behavior. Quick asset-folder selection
uses AppModel::set_selected_root. Other fields use the Settings save action.
Diagnostics are evaluated only while this section is visible, never by the rail.

The section heading and Layers terrain-setting tooltip use Files & Cache, matching
the unified settings navigation.
