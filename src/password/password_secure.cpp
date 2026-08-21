#include "sart/password/secure.hpp"

#include "sart/visual/art.hpp"

#include <atomic>
#include <cerrno>
#include <cstring>
#include <stdexcept>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <system_error>
#include <unistd.h>
#include <utility>

namespace sart::password {

    void protect_process_secrets() {
        if (prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) != 0) {
            throw std::system_error(errno, std::generic_category(), "disable process dumpability");
        }
    }

    SecureSecret::SecureSecret(std::size_t capacity) : capacity_(capacity) {
        if (capacity == 0 || capacity > maximum_secret_bytes) {
            throw std::invalid_argument("secret capacity must be in 1..=4096");
        }
        const auto queried = sysconf(_SC_PAGESIZE);
        const auto page_size = queried > 0 ? static_cast<std::size_t>(queried) : 4096;
        mapping_length_ = (capacity + page_size - 1) / page_size * page_size;
        auto *mapping = mmap(nullptr, mapping_length_, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (mapping == MAP_FAILED) {
            throw std::system_error(errno, std::generic_category(), "allocate protected secret");
        }
        data_ = static_cast<std::byte *>(mapping);
        protection_.locked = mlock(data_, mapping_length_) == 0;
        protection_.excluded_from_core = madvise(data_, mapping_length_, MADV_DONTDUMP) == 0;
    }

    SecureSecret::SecureSecret(SecureSecret &&other) noexcept
        : data_(std::exchange(other.data_, nullptr)), length_(std::exchange(other.length_, 0)),
          capacity_(std::exchange(other.capacity_, 0)), mapping_length_(std::exchange(other.mapping_length_, 0)),
          protection_(std::exchange(other.protection_, {})) {}

    SecureSecret &SecureSecret::operator=(SecureSecret &&other) noexcept {
        if (this != &other) {
            release();
            data_ = std::exchange(other.data_, nullptr);
            length_ = std::exchange(other.length_, 0);
            capacity_ = std::exchange(other.capacity_, 0);
            mapping_length_ = std::exchange(other.mapping_length_, 0);
            protection_ = std::exchange(other.protection_, {});
        }
        return *this;
    }

    SecureSecret::~SecureSecret() { release(); }
    std::size_t SecureSecret::size() const noexcept { return length_; }
    bool SecureSecret::empty() const noexcept { return length_ == 0; }
    std::size_t SecureSecret::capacity() const noexcept { return capacity_; }
    SecretProtection SecureSecret::protection() const noexcept { return protection_; }

    void SecureSecret::push(char32_t character) { push(encode_utf8(character)); }

    void SecureSecret::push(std::string_view text) {
        static_cast<void>(decode_utf8(text));
        if (text.size() > capacity_ - length_) {
            throw std::length_error("secret exceeds its byte limit");
        }
        std::memcpy(data_ + length_, text.data(), text.size());
        length_ += text.size();
    }

    std::optional<char32_t> SecureSecret::pop() {
        if (empty())
            return std::nullopt;
        const auto view = std::string_view(reinterpret_cast<const char *>(data_), length_);
        const auto characters = decode_utf8(view);
        if (characters.empty())
            throw std::runtime_error("secret input is not valid UTF-8");
        const auto removed = characters.back();
        const auto bytes = encode_utf8(removed).size();
        zero_range(length_ - bytes, length_);
        length_ -= bytes;
        return removed;
    }

    void SecureSecret::clear() noexcept {
        zero_range(0, mapping_length_);
        length_ = 0;
    }

    std::span<std::byte> SecureSecret::receive_buffer() noexcept { return {data_, capacity_}; }

    void SecureSecret::commit_received(std::size_t length) {
        if (length > capacity_) {
            clear();
            throw std::length_error("received secret exceeds its byte limit");
        }
        try {
            static_cast<void>(decode_utf8(std::string_view(reinterpret_cast<const char *>(data_), length)));
        } catch (...) {
            clear();
            throw std::runtime_error("received secret is not valid UTF-8");
        }
        length_ = length;
        zero_range(length, mapping_length_);
    }

    void SecureSecret::release() noexcept {
        if (!data_)
            return;
        clear();
        if (protection_.locked)
            static_cast<void>(munlock(data_, mapping_length_));
        static_cast<void>(munmap(data_, mapping_length_));
        data_ = nullptr;
        capacity_ = 0;
        mapping_length_ = 0;
    }

    void SecureSecret::zero_range(std::size_t begin, std::size_t end) noexcept {
        std::atomic_signal_fence(std::memory_order_seq_cst);
        volatile std::byte *bytes = data_;
        for (auto index = begin; index < end; ++index)
            bytes[index] = std::byte{};
        std::atomic_signal_fence(std::memory_order_seq_cst);
    }

} // namespace sart::password
