# literals.rs

Decodes the two static data representations used by the publisher's
bundler. Large tables use JSON in JavaScript strings. Small tables use an
array of plain objects with bare keys. String escapes and bracket boundaries
are handled before serde checks the JSON. No code from the page is executed.
