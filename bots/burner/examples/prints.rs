//! Does nothing and burns every tick's fuel on `burner::Prints` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::Prints>);
