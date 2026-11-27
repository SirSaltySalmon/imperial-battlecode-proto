//! Does nothing and burns every tick's fuel on `burner::Logs` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::Logs>);
