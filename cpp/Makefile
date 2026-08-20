SHELL := /bin/bash

CXX ?= c++
AR ?= ar
MODE ?= debug
BUILD_DIR ?= ../target/cpp/$(MODE)

CPPFLAGS := -Iinclude
COMMON_FLAGS := -std=c++23 -Wall -Wextra -Wpedantic -Werror -pthread
DEBUG_FLAGS := -Og -g3
RELEASE_FLAGS := -Os -DNDEBUG -ffunction-sections -fdata-sections -fno-ident
ifeq ($(MODE),release)
CXXFLAGS := $(COMMON_FLAGS) $(RELEASE_FLAGS)
LDFLAGS := -pthread -static -Wl,--gc-sections -Wl,--build-id=none -s
else
CXXFLAGS := $(COMMON_FLAGS) $(DEBUG_FLAGS)
LDFLAGS := -pthread
endif
CODEC_LIBS ?= -lz -lzstd

LIBRARY_SOURCES := $(filter-out src/main.cpp,$(wildcard src/*.cpp) $(wildcard src/splash/*.cpp))
MAIN_SOURCE := src/main.cpp
TEST_SOURCES := $(wildcard tests/*.cpp)
LIBRARY_OBJECTS := $(patsubst %.cpp,$(BUILD_DIR)/%.o,$(LIBRARY_SOURCES))
MAIN_OBJECT := $(BUILD_DIR)/$(MAIN_SOURCE:.cpp=.o)
TEST_OBJECTS := $(patsubst %.cpp,$(BUILD_DIR)/%.o,$(TEST_SOURCES))
DEPENDENCIES := $(LIBRARY_OBJECTS:.o=.d) $(MAIN_OBJECT:.o=.d) $(TEST_OBJECTS:.o=.d)
LIBRARY := $(BUILD_DIR)/libbootart.a
BINARY := $(BUILD_DIR)/bootart
TEST_BINARY := $(BUILD_DIR)/bootart-tests

.PHONY: build test clean

build: $(BINARY)

test: $(TEST_BINARY) $(BINARY)
	@BOOTART_BINARY='$(abspath $(BINARY))' '$(TEST_BINARY)'

$(BINARY): $(MAIN_OBJECT) $(LIBRARY)
	@mkdir -p '$(@D)'
	$(CXX) $(CXXFLAGS) $^ $(LDFLAGS) $(CODEC_LIBS) -o '$@'

$(TEST_BINARY): $(TEST_OBJECTS) $(LIBRARY)
	@mkdir -p '$(@D)'
	$(CXX) $(CXXFLAGS) $^ $(LDFLAGS) $(CODEC_LIBS) -o '$@'

$(LIBRARY): $(LIBRARY_OBJECTS)
	@mkdir -p '$(@D)'
	$(AR) rcs '$@' $^

$(BUILD_DIR)/tests/%.o: tests/%.cpp
	@mkdir -p '$(@D)'
	$(CXX) $(CPPFLAGS) $(CXXFLAGS) -DBOOTART_SOURCE_ROOT='"$(abspath ..)"' -MMD -MP -c '$<' -o '$@'

$(BUILD_DIR)/%.o: %.cpp
	@mkdir -p '$(@D)'
	$(CXX) $(CPPFLAGS) $(CXXFLAGS) -MMD -MP -c '$<' -o '$@'

clean:
	rm -rf '$(BUILD_DIR)'

-include $(DEPENDENCIES)
