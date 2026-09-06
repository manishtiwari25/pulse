// The hub has no projection database or dashboard state directory. It renders
// immutable posts through tally.Records, records through store.GetRecord, and
// owner writes through the same board/tasks/store helpers as the CLI.
//
// Compatibility differences required by the Go migration:
//
//   - Project presence means a directory in a Git checkout. The old requirement
//     for docs/ is deliberately gone (D-L); an unborn Git checkout is valid.
//   - Metadata reports the store path, schema/records versions, canonical binary
//     version and generation time. Compatibility keys remain where the UI reads
//     them; fingerprint is store:<mtime_ns>:<size>:<max event seq>, not a
//     disposable projection hash.
//   - Evolution uses the approved SQL-native constitution/evaluators/archive/loop
//     document, not Python's shell-report shape. Record slugs constitution,
//     evaluators and archive carry the historical texts; evolution_archive holds
//     live experiment rows when Phase 3's additive table exists. No fixture names
//     or private directory paths are returned and no scorer or fixture is read.
//   - Settings include compute.* and evolve.*. Public settings use secret:false;
//     the private held-out directory and historical credential-shaped values use
//     secret:true. The directory is configurable but never returned: settings
//     reads/POST echoes and setting events (polling/SSE) mask it without changing
//     the stored value or event. Timeline buckets contain no setting details.
//     The shared bus privacy policy also scrubs current and historical private
//     values from other event subjects/details, including JSON encodings. Live
//     SSE refreshes that policy per event so changes and clears remain private.
//     Settings are organization-wide: schema 3 has no project override storage,
//     so every per-project settings route returns the organization values.
//   - A registered project without an indexed constitution returns a 200 empty
//     document (record:null, body:"", amendments:[]); an unknown project is 404.
//
// testdata/golden contains the Python dashboard responses on the parity
// snapshot. Its actual project prefix was /p/<slug>/; all three project prefixes
// are accepted here. The original Python project settings route raises a
// Path.home collision (q_settings mistakes the method for a directory), so those
// fixtures use the identical intended q_settings result from the org route.
// Golden tests list every ignored clock/tally key and separately report native
// metadata/settings differences. Evolution fields are checked against the new
// contract, not against Python's filesystem-derived report. All other values and
// array order must match the oracle.
//
// Parity exceptions (P = /p/<slug>/api/v1, also served at the new API prefix):
//
//	Route       | Python behavior                 | Go behavior
//	P/settings  | TypeError from Path.home        | Organization-wide owner settings
//	P/evolution | Filesystem/shell-report shape   | SQL-native records/archive/loop
//	P/project   | Disposable projection metadata | Actual store/build identity
//	/api/v1/org | projects.json registry path     | Actual SQLite store path
//	*/settings  | Legacy setting whitelist       | Legacy then effective extras
package hub
