//! Does nothing and burns every tick's fuel on `burner::Reads` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::Reads>);
