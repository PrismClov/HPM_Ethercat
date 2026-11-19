/*
 * Copyright (c) 2024 HPMicro
 *
 * SPDX-License-Identifier: BSD-3-Clause
 *
 */

#include <stdio.h>
#include "board.h"
#include "ecat_def.h"
#include "ecatappl.h"
#include "ecatslv.h"
#include "applInterface.h"
#include "digital_io.h"
#include "hpm_ecat_hw.h"
#include "hpm_l1c_drv.h"
#include "oled.h"

volatile uint32_t p0_diagnostic[9] __attribute__((section(".noncacheable.bss")));

static void record_p0_phy(void)
{
    static const uint8_t reg[] = {2, 3, 1, 1};
    const uint8_t phy = BOARD_ECAT_PHY_ADDR_OFFSET + BOARD_ECAT_PORT0_PHY_ADDR;
    for (unsigned i = 0; i < sizeof(reg); ++i) {
        uint16_t value = 0;
        p0_diagnostic[1 + i * 2] = esc_mdio_read(HPM_ESC, phy, reg[i], &value);
        p0_diagnostic[2 + i * 2] = value;
    }
    p0_diagnostic[0] = 0x50485904;
}

int main(void)
{
    hpm_stat_t stat;
    p0_diagnostic[0] = 0x50485901;
    board_init();
    board_init_led_pins();
    OLED_Init();
    OLED_ColorTurn(0);    //0正常显示，1 反色显示
    OLED_DisplayTurn(0);  //0正常显示 1 屏幕翻转显示
    uint8_t title[] = "[EtherCAT]";
    uint8_t input_label[] = "InputCounter";
    OLED_ShowString(20, 0, title, 16, 1);
    OLED_ShowString(0, 16, input_label, 16, 1);
    OLED_Refresh();

    board_init_ethercat(HPM_ESC); /* init ESC function pins */
    printf("EtherCAT IO sample\n");

    /* hardware init */
    p0_diagnostic[0] = 0x50485902;
    stat = ecat_hardware_init(HPM_ESC);
    p0_diagnostic[1] = stat;
    p0_diagnostic[0] = 0x50485903;
    if (stat != status_success) {
        printf("Init ESC peripheral and related devices(EEPROM/PHY) failed!\n");
        return 0;
    }

    record_p0_phy();
    MainInit(); /* SSC Initilize the stack */

#if defined(ESC_EEPROM_EMULATION) && ESC_EEPROM_EMULATION
    pAPPL_EEPROM_Read  = ecat_eeprom_emulation_read;
    pAPPL_EEPROM_Write = ecat_eeprom_emulation_write;
    pAPPL_EEPROM_Reload = ecat_eeprom_emulation_reload;
    pAPPL_EEPROM_Store  = ecat_eeprom_emulation_store;
#endif

    /* Create basic mapping */
    APPL_GenerateMapping(&nPdInputSize, &nPdOutputSize);

    /* Set stack run flag */
    bRunApplication = TRUE;
    /* Execute the stack */
    while (bRunApplication == TRUE) {
        MainLoop();
    }

    /* hardware deinit */

    return 0;
}
