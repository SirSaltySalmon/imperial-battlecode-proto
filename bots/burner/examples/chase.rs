//! Does nothing and burns every tick's fuel on `burner::Chase` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::Chase>);
