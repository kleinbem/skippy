# Legacy runtime manifest fixture

`native-runtimes-v0.78.0.json` is the unchanged historical release manifest used
to test compatibility with the unversioned manifest schema. Its `mesh_version`
fields, artifact IDs and publication URLs are deliberate fixture data. Tests
read it locally and do not download those URLs. New runtime packages use schema
2, `release_version`, and Skippy-owned artifact names.
