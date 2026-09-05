# Require frozen inputs for v1

Version 1 inspects only a stopped database, immutable snapshot, or otherwise stable copy and invalidates an inspection when its input changes. A raw scan of changing SQLite files cannot promise one transaction-consistent state—especially around WAL and rollback journals—so live acquisition and follow mode remain separate future capabilities rather than weakening the meaning of a database snapshot.
