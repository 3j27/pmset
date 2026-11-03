PREFIX ?= /usr/local
DESTDIR ?=
CARGO ?= cargo

BINDIR := $(DESTDIR)$(PREFIX)/bin
MANDIR := $(DESTDIR)$(PREFIX)/share/man/man1

.PHONY: all build test fmt clippy install uninstall clean

all: build

build:
	$(CARGO) build --release

test:
	$(CARGO) test

fmt:
	$(CARGO) fmt

clippy:
	$(CARGO) clippy --all-targets -- -D warnings

install: build
	install -Dm755 target/release/pmset $(BINDIR)/pmset
	install -Dm644 pmset.1 $(MANDIR)/pmset.1

uninstall:
	rm -f $(BINDIR)/pmset
	rm -f $(MANDIR)/pmset.1

clean:
	$(CARGO) clean
