//! Does nothing and burns every tick's fuel on `burner::MemRead` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::MemRead>);
