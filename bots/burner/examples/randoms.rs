//! Does nothing and burns every tick's fuel on `burner::Randoms` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::Randoms>);
