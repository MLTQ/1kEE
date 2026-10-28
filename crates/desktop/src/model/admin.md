# admin.rs

## Purpose
Own session-only admin level, shading and opacity preferences. Visibility remains
`AppModel::show_admin`; hiding the parent retains all child selections.

## Contracts
- Levels 2–11 are independently selectable; 2/4/6/8 default on for compatibility.
- Labels are examples: division names and level assignments vary by country.
- Shading defaults off with 16% opacity when enabled. Data availability does not
  silently change selections. An empty selection requests no cache work.
