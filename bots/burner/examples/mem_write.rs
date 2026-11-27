//! Does nothing and burns every tick's fuel on `burner::MemWrite` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::MemWrite>);
