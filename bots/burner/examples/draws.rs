//! Does nothing and burns every tick's fuel on `burner::Draws` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::Draws>);
