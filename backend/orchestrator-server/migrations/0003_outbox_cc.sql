-- An email can copy people in; one send stays one message, so the copies live on its row.
ALTER TABLE outbox_messages ADD COLUMN cc TEXT NOT NULL DEFAULT '[]'; -- JSON array of addresses
