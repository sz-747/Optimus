# Dashboard Reference

The design source for the migration's first Tauri shell is the mission-control dashboard currently
implemented at `app/Dashboard/DashboardView.cs`. It was built from the supplied reference image;
the legacy implementation is a visual reference only and must not become a new WinUI feature
surface.

The P0 frontend preserves these compositional anchors:

- an integrated 54px title bar;
- a 338px left rail with safe capacity, workspace creation, grouped workspaces, and settings;
- selected-workspace heading plus four summary cards;
- agent graph beside a chronological activity feed;
- timeline and optional technical-details row.

Use `DESIGN.md` for legacy token names and `docs/design/web-overlay-palette/tokens.css` for the
web token source. P4 binds the capacity meter, workspace rows, and controls to the Rust backend;
P0 renders only the static composition.
