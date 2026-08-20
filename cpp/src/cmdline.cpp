#include "bootart/cmdline.hpp"

#include <fstream>
#include <stdexcept>
#include <string>

namespace bootart::cmdline {

    bool splash_disabled(std::string_view command_line) noexcept {
        while (!command_line.empty()) {
            const auto separator = command_line.find_first_of(" \t\r\n");
            const auto token = command_line.substr(0, separator);
            if (token == "bootart=0" || token == "rd.bootart=0") {
                return true;
            }
            if (separator == std::string_view::npos) {
                break;
            }
            command_line.remove_prefix(separator + 1);
        }
        return false;
    }

    bool splash_disabled_at(const std::filesystem::path &path) {
        std::ifstream input(path, std::ios::binary);
        if (!input) {
            throw std::runtime_error("cannot read kernel command line");
        }
        std::string contents{std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
        return splash_disabled(contents);
    }

    bool early_boot_enabled_at(const std::filesystem::path &path) noexcept {
        try {
            return !splash_disabled_at(path);
        } catch (...) {
            return false;
        }
    }

} // namespace bootart::cmdline
