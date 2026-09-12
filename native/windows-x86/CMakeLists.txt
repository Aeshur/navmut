cmake_minimum_required(VERSION 3.20)

project(navmut_native LANGUAGES CXX)

include(CTest)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

add_library(navmut-native-core STATIC
    src/protocol.cpp
    src/shared_state.cpp
    src/validation.cpp
)
target_include_directories(navmut-native-core PUBLIC include)
target_compile_features(navmut-native-core PUBLIC cxx_std_20)

if(MSVC)
    target_compile_options(navmut-native-core PRIVATE /W4 /EHsc /permissive-)
    set_target_properties(navmut-native-core PROPERTIES MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
else()
    target_compile_options(navmut-native-core PRIVATE -Wall -Wextra -Wpedantic)
endif()

add_executable(navmut-native-tests tests/native_tests.cpp)
target_link_libraries(navmut-native-tests PRIVATE navmut-native-core)
if(MSVC)
    target_compile_options(navmut-native-tests PRIVATE /W4 /EHsc /permissive-)
    set_target_properties(navmut-native-tests PROPERTIES MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
else()
    target_compile_options(navmut-native-tests PRIVATE -Wall -Wextra -Wpedantic)
endif()
add_test(NAME navmut-native-tests COMMAND navmut-native-tests)

if(WIN32)
    add_library(navmut-helper-hook SHARED src/hook_dll.cpp src/hook_dll.def)
    target_include_directories(navmut-helper-hook PRIVATE include)
    target_link_libraries(navmut-helper-hook PRIVATE navmut-native-core user32 kernel32)
    target_compile_definitions(navmut-helper-hook PRIVATE NAVMUT_HELPER_DLL=1)

    add_executable(navmut-helper src/controller.cpp src/helper_main.cpp)
    target_include_directories(navmut-helper PRIVATE include)
    target_link_libraries(navmut-helper PRIVATE navmut-native-core user32 kernel32 advapi32 bcrypt)

    # The retail client and SetWindowsHookEx target are both 32-bit. A
    # Win32 CMake generator (cmake -A Win32) is required for this target.
    foreach(target navmut-helper navmut-helper-hook)
        target_compile_definitions(${target} PRIVATE WIN32_LEAN_AND_MEAN NOMINMAX _WIN32_WINNT=0x0601)
        if(MSVC)
            target_compile_options(${target} PRIVATE /W4 /EHsc /permissive- /arch:IA32)
            set_target_properties(${target} PROPERTIES MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
        endif()
    endforeach()
    set_target_properties(navmut-helper PROPERTIES OUTPUT_NAME "navmut-helper")
    set_target_properties(navmut-helper-hook PROPERTIES OUTPUT_NAME "navmut-helper-hook")
    install(TARGETS navmut-helper navmut-helper-hook
        RUNTIME DESTINATION navmut
        LIBRARY DESTINATION navmut
    )
else()
    message(STATUS "Native helper targets are Windows-only; building protocol tests")
endif()
