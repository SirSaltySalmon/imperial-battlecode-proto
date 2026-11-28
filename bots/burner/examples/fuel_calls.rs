//! Does nothing and burns every tick's fuel on `burner::FuelCalls` (see its doc).
ibc::bot!(burner::Burner<burner::Idle, burner::FuelCalls>);
