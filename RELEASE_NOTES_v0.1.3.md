# PhotoMind 0.1.3

- Moments now group photos captured close together even when the scene changes. A moment can span up to 15 minutes, with no more than three minutes between consecutive photos.
- People suggestions require stronger agreement across every face in a group, preventing a chain of weak matches from mixing several people into one large group.
- Very small face crops are excluded from identity suggestions because they do not contain enough detail for reliable matching. They still appear in the detected face count.

All analysis runs on the device with bundled models. No account, LLM, or external service is needed. Review suggested people and duplicates before deleting photos.
