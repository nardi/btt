//! Checks that every model type exports to TypeScript.

use btt_model::{StoredTrip, TripEdit, TripEditError, TripEditOutcome, TripSummary};
use specta::Types;
use specta_typescript::Typescript;

#[test]
fn model_types_export_to_typescript() {
    let types = Types::default()
        .register::<StoredTrip>()
        .register::<TripSummary>()
        .register::<TripEdit>()
        .register::<TripEditOutcome>()
        .register::<TripEditError>();
    let typescript = Typescript::default()
        .export(&types, specta_serde::Format)
        .unwrap();
    println!("{typescript}");
    assert!(typescript.contains("export type Trip = "));
    assert!(typescript.contains("export type Itinerary = "));
}
