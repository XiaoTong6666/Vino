# SPDX-License-Identifier: GPL-2.0
#
# Top-level build for the userspace Vino service. The display kernel module and
# libevdi come from the stock EVDI project in /home/xiaotong/evdi (or the
# system installation); this tree owns only the dock-side implementation.

CARGO ?= cargo
EVDI_DIR ?= ../evdi

all: chimera

# The complete EVDI-to-Vino userspace service.
chimera: evdi
	EVDI_LIB_DIR=$(abspath $(EVDI_DIR)/library) \
		$(CARGO) build --release -p vino-chimera --bin chimera --features live

evdi:
	$(MAKE) -C $(EVDI_DIR)/library

# The offline proof: replays real captured DLM sessions through the vendored
# kernel control-plane code and asserts byte-exact equivalence. No hardware.
test:
	python3 -c 'compile(open("scripts/hardware_test.py", encoding="utf-8").read(), "scripts/hardware_test.py", "exec")'
	$(CARGO) test --workspace

lint: evdi
	EVDI_LIB_DIR=$(abspath $(EVDI_DIR)/library) \
		LD_LIBRARY_PATH=$(abspath $(EVDI_DIR)/library) \
		$(CARGO) clippy --workspace --all-targets \
			--features vino-chimera/live -- -D warnings

clean:
	$(MAKE) -C $(EVDI_DIR)/library clean
	$(CARGO) clean

.PHONY: all chimera evdi test lint clean
