#ifndef MONITOR_HPM_PDO_H
#define MONITOR_HPM_PDO_H

#include "protocol.h"

#define HPM_VENDOR 0x00024121u
#define HPM_PRODUCT 0x00000001u
#define HPM_OUTPUT 0x7010
#define HPM_INPUT 0x6000
#define HPM_PDO_BYTES 4

typedef size_t (*hpm_read_sdo)(void *master, uint16_t index, uint8_t subindex,
                               uint8_t *bytes, size_t capacity);

static uint32_t hpm_u32(const uint8_t *bytes)
{
    return (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) |
           ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
}

static int hpm_assignment(void *master, hpm_read_sdo read, uint16_t assignment,
                          uint16_t mapping, uint16_t object)
{
    uint8_t bytes[HPM_PDO_BYTES] = {0};
    size_t size = read(master, assignment, 0, bytes, sizeof(bytes));
    if ((size != 1 && size != 2) || hpm_u32(bytes) != 1)
        return 0;
    if (read(master, assignment, 1, bytes, sizeof(bytes)) != 2 ||
        ((unsigned)bytes[0] | ((unsigned)bytes[1] << 8)) != mapping)
        return 0;
    for (size_t i = 0; i < sizeof(bytes); ++i)
        bytes[i] = 0;
    size = read(master, mapping, 0, bytes, sizeof(bytes));
    if ((size != 1 && size != 2) || hpm_u32(bytes) != 1)
        return 0;
    return read(master, mapping, 1, bytes, sizeof(bytes)) == sizeof(bytes) &&
           hpm_u32(bytes) == (((uint32_t)object << 16) | 32u);
}

static int hpm_mapping_matches(void *master, hpm_read_sdo read)
{
    return hpm_assignment(master, read, 0x1c12, 0x1600, HPM_OUTPUT) &&
           hpm_assignment(master, read, 0x1c13, 0x1a00, HPM_INPUT);
}

static void pdo_row(int valid, int wkc, int expected, const uint8_t *input)
{
    printf("ecat\tpdo\t%d\t%d\t", wkc, expected);
    if (valid)
        printf("%08" PRIx32, hpm_u32(input));
    putchar('\n');
}

static void pdo_error(const char *message)
{
    printf("ecat\terror\t%s\n", message);
    fflush(stdout);
}

#endif
