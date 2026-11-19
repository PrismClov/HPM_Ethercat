cmake_minimum_required(VERSION 3.13)
set(HPM_BUILD_TYPE ram)
set(BOARD hpm5e00kit CACHE STRING "HPM board")
set(BOARD_SEARCH_PATH "${CMAKE_CURRENT_LIST_DIR}/../boards" CACHE PATH "Custom board directory")
find_package(hpm-sdk REQUIRED HINTS $ENV{HPM_SDK_BASE})
project(phy_diag)
sdk_app_src(phy_diag.c)

sdk_compile_definitions(-DCONFIG_NDEBUG_CONSOLE=1 -DBOARD_SHOW_CLOCK=0 -DBOARD_SHOW_BANNER=0)
