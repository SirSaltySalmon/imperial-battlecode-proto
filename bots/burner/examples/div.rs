//! Does nothing and burns every tick's fuel on `burner::Div` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::Div>);
