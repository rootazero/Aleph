BINARY := whatsapp-bridge
PKG    := ./cmd/whatsapp-bridge

# Release build: strip debug symbols + DWARF + build path info.
# Cuts ~30% off the default go-build output.
RELEASE_LDFLAGS := -s -w

.PHONY: all build release debug strip clean test

# Default target = release (this is what gets shipped with Aleph).
all: release

release:
	go build -ldflags "$(RELEASE_LDFLAGS)" -trimpath -o $(BINARY) $(PKG)
	@echo "built $(BINARY) ($$(du -h $(BINARY) | cut -f1))"

# Debug build keeps symbols for backtraces / delve.
debug:
	go build -o $(BINARY) $(PKG)
	@echo "built $(BINARY) with debug symbols ($$(du -h $(BINARY) | cut -f1))"

# Strip an already-built binary in place (no rebuild).
strip:
	@if [ ! -f $(BINARY) ]; then echo "$(BINARY) not found, run 'make' first"; exit 1; fi
	go tool strip $(BINARY) 2>/dev/null || strip $(BINARY)
	@echo "stripped $(BINARY) ($$(du -h $(BINARY) | cut -f1))"

clean:
	rm -f $(BINARY)

test:
	go test ./...
