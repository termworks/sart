#include "sart/core/signals.hpp"

#include <doctest/doctest.h>

#include <csignal>
#include <stdexcept>
#include <utility>

TEST_SUITE("core signals") {

    TEST_CASE("stop flag resets to false") {
        sart::core::signals::reset_stop_flag();
        CHECK_FALSE(sart::core::signals::should_stop());
    }

    TEST_CASE("SIGTERM requests a stop") {
        sart::core::signals::reset_stop_flag();
        {
            sart::core::signals::SignalGuard guard;
            REQUIRE(std::raise(SIGTERM) == 0);
            CHECK(sart::core::signals::should_stop());
        }
        sart::core::signals::reset_stop_flag();
    }

    TEST_CASE("SIGINT requests a stop") {
        sart::core::signals::reset_stop_flag();
        {
            sart::core::signals::SignalGuard guard;
            REQUIRE(std::raise(SIGINT) == 0);
            CHECK(sart::core::signals::should_stop());
        }
        sart::core::signals::reset_stop_flag();
    }

    TEST_CASE("only one signal guard may be active") {
        sart::core::signals::SignalGuard guard;
        CHECK_THROWS_AS(static_cast<void>(sart::core::signals::SignalGuard{}), std::runtime_error);
    }

    TEST_CASE("moving a signal guard transfers ownership") {
        sart::core::signals::reset_stop_flag();
        {
            sart::core::signals::SignalGuard source;
            sart::core::signals::SignalGuard destination(std::move(source));
            REQUIRE(std::raise(SIGTERM) == 0);
            CHECK(sart::core::signals::should_stop());
        }
        CHECK_NOTHROW(static_cast<void>(sart::core::signals::SignalGuard{}));
        sart::core::signals::reset_stop_flag();
    }

} // TEST_SUITE
