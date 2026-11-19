#ifndef MONITOR_PROTOCOL_H
#define MONITOR_PROTOCOL_H

#include <inttypes.h>
#include <stdio.h>

static void hex_field(const char *text, size_t capacity)
{
    for (size_t i = 0; i < capacity && text[i]; ++i)
        printf("%02x", (unsigned)(unsigned char)text[i]);
}

/* One snapshot per request. Hex encoding keeps device names out of framing. */
static void slave_row(unsigned position, uint32_t vendor, uint32_t product,
                      uint32_t revision, int state, int al_code,
                      const char *name, size_t capacity)
{
    printf("ecat\tslave\t%u\t%08" PRIx32 "\t%08" PRIx32 "\t%08" PRIx32 "\t%d\t%d\t",
           position, vendor, product, revision, state, al_code);
    hex_field(name, capacity);
    putchar('\n');
}

static int next_request(void)
{
    char request[16];
    if (!fgets(request, sizeof(request), stdin))
        return 0;
    return request[0] == 'S' || request[0] == 'P' ? request[0] : 0;
}

static void end_snapshot(void)
{
    puts("ecat\tend");
    fflush(stdout);
}

#endif
