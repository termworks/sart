set_project("sart")
set_version("0.1.0")
set_xmakever("2.8.5")

add_rules("mode.debug", "mode.release")
set_languages("c++23")
set_policy("package.requires_lock", true)
set_policy("build.fence", true)
set_warnings("all", "extra", "pedantic", "error")
set_config("builddir", "target/xmake")

option("tests")
    set_default(true)
    set_showmenu(true)
    set_description("Build the doctest suite")
option_end()

option("musl")
    set_default(false)
    set_showmenu(true)
    set_description("Write release artifacts to the musl output directory")
option_end()

local output_mode = has_config("musl") and "musl" or (is_mode("release") and "release" or "debug")
local project_root = os.scriptdir()

local function configure_cpp_target()
    add_includedirs("include", {public = true})
    if has_config("musl") then
        if os.getenv("SART_MUSL_ZLIB") then
            add_linkdirs(path.join(os.getenv("SART_MUSL_ZLIB"), "lib"))
        end
        if os.getenv("SART_MUSL_ZSTD") then
            add_linkdirs(path.join(os.getenv("SART_MUSL_ZSTD"), "lib"))
        end
    end
    on_load(function(target)
        import("core.project.project")
        target:add("defines", 'SART_VERSION="' .. project.version() .. '"')
    end)
    add_cxxflags("-pthread")
    if is_mode("release") then
        set_optimize("smallest")
        add_defines("NDEBUG")
        add_cxxflags("-ffunction-sections", "-fdata-sections", "-fno-ident")
    else
        set_symbols("debug")
        set_optimize("none")
        add_cxxflags("-Og")
    end
end

target("sart-core")
    set_kind("static")
    set_default(false)
    set_filename("libsart.a")
    set_targetdir("target/cpp/" .. output_mode)
    add_files("src/**.cpp")
    remove_files("src/main.cpp")
    configure_cpp_target()
target_end()

target("sart")
    set_kind("binary")
    set_default(true)
    set_targetdir("target/cpp/" .. output_mode)
    add_deps("sart-core")
    add_files("src/main.cpp")
    add_syslinks("pthread", "z", "zstd")
    configure_cpp_target()
    if is_mode("release") then
        add_ldflags("-static", "-Wl,--gc-sections", "-Wl,--build-id=none", "-s", {force = true})
    end
target_end()

if has_config("tests") then
    target("sart-tests")
        set_kind("binary")
        set_default(false)
        set_targetdir("target/cpp/" .. output_mode)
        set_rundir("$(projectdir)")
        add_deps("sart", "sart-core")
        add_files("tests/*.cpp")
        if os.getenv("DOCTEST_INCLUDE_DIR") then
            add_includedirs(os.getenv("DOCTEST_INCLUDE_DIR"), {external = true})
        end
        add_defines('SART_SOURCE_ROOT="$(projectdir)"')
        add_syslinks("pthread", "z", "zstd")
        configure_cpp_target()
        add_tests("doctest", {
            runenvs = {SART_BINARY = project_root .. "/target/cpp/" .. output_mode .. "/sart"},
            timeout = 120,
            realtime_output = true,
        })
    target_end()
end

includes("xmake/tasks/common.lua", "xmake/tasks/artifacts.lua", "xmake/tasks/vm.lua")
