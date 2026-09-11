set positional-arguments

# Create catalog, WAL, and damaged demos; existing frozen examples are reused.
generate:
    python3 examples/create_demo.py

# Serve catalog (default), wal, or damaged on all interfaces at port 3000.
web name="catalog": generate
    just user web "target/examples/$1/database.sqlite" --semantic-metadata

# Open catalog (default), wal, or damaged in the terminal; q exits.
terminal name="catalog": generate
    just user terminal "target/examples/$1/database.sqlite"

# Explain what to inspect in each demo.
guide:
    @cat examples/README.md
