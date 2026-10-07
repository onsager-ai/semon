# Catalog source change tokens

Native source observations retain the file generation captured with their
consumed-prefix ledger. On Unix this includes device, inode, size, modification
time, and change time. Schema 14 adds a nullable change-time column without
discarding existing source records or retained history projections.

Change time prevents a same-size rewrite with a restored modification time from
serving new bytes through old transcript recipes. Local range reads compare the
captured generation before and after reading. A mismatch produces an incomplete
observation until the producer rebuilds the projection; it does not fall back to
an archived reader for a present, changed source. Background refresh revalidates
the consumed prefix and publishes a new generation.

Old observations have no change-time proof. They remain readable as metadata,
but cannot authorize verified local body reads until republished. Platforms
without a change-time token use the same conservative rule. Authorized provider
reads may still serve original bytes by their consumed-prefix proof. Neither
file generation nor observation freshness grants credential or native control
authority.

The regression restores modification time after an in-place, equal-length
rewrite, verifies that cached local ranges become incomplete, and confirms that
producer refresh exposes the new body under a new projection generation.
