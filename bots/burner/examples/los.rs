//! Does nothing and burns every tick's fuel on `burner::Los` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::Los>);
