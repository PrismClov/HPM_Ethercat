#ifndef MONITOR_PROCESS_IMAGE_H
#define MONITOR_PROCESS_IMAGE_H

#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include "protocol.h"

#define IMAGE_MAX_ENTRIES 16384u

typedef struct
{
    unsigned station, output, pdo, index, subindex, bits;
    size_t offset;
    char name[64];
} image_entry;

typedef struct
{
    image_entry entries[IMAGE_MAX_ENTRIES];
    size_t count;
    uint8_t *data;
    size_t bytes;
    int valid;
} process_image;

static int image_options(int argc, char **argv, uint32_t *period, unsigned *assign)
{
    char *end;
    if (argc != 5 || strcmp(argv[2], "--process") != 0)
        return 0;
    errno = 0;
    unsigned long ns = strtoul(argv[3], &end, 10);
    if (errno || !argv[3][0] || *end || ns < 100000 || ns > 20000000)
        return 0;
    unsigned long activation = strtoul(argv[4], &end, 10);
    if (errno || !argv[4][0] || *end || activation > UINT16_MAX)
        return 0;
    *period = (uint32_t)ns;
    *assign = (unsigned)activation;
    return 1;
}

static int image_add(process_image *image, unsigned station, unsigned output,
                     unsigned pdo, unsigned index, unsigned subindex,
                     unsigned bits, size_t offset, const char *name)
{
    if (!index)
        return 1; /* Padding occupies bits but has no variable. */
    if (!bits || image->count >= IMAGE_MAX_ENTRIES)
        return 0;
    image_entry *entry = &image->entries[image->count++];
    *entry = (image_entry){station, output, pdo, index, subindex, bits, offset, {0}};
    if (name)
        snprintf(entry->name, sizeof(entry->name), "%s", name);
    return 1;
}

static int image_layout(process_image *image)
{
    if (!image->count || !image->data || !image->bytes)
        return 0;
    for (size_t i = 0; i < image->count; ++i)
    {
        const image_entry *e = &image->entries[i];
        if (e->offset > image->bytes * 8 || e->bits > image->bytes * 8 - e->offset)
            return 0;
    }
    for (size_t i = 0; i < image->count; ++i)
    {
        const image_entry *e = &image->entries[i];
        printf("ecat\tentry\t%zu\t%u\t%u\t%u\t%u\t%u\t%u\t%zu\t",
               i, e->station, e->output, e->pdo, e->index, e->subindex, e->bits, e->offset);
        for (const char *p = e->name; *p; ++p)
            printf("%02x", (unsigned)(unsigned char)*p);
        putchar('\n');
    }
    end_snapshot();
    return 1;
}

static uint64_t image_get(const process_image *image, const image_entry *entry)
{
    uint64_t value = 0;
    for (unsigned i = 0; i < entry->bits; ++i)
    {
        size_t bit = entry->offset + i;
        value |= (uint64_t)((image->data[bit / 8] >> (bit % 8)) & 1u) << i;
    }
    return value;
}

static void image_set(process_image *image, const image_entry *entry, uint64_t value)
{
    for (unsigned i = 0; i < entry->bits; ++i)
    {
        size_t bit = entry->offset + i;
        uint8_t mask = (uint8_t)(1u << (bit % 8));
        if (i < 64 && ((value >> i) & 1u))
            image->data[bit / 8] |= mask;
        else
            image->data[bit / 8] &= (uint8_t)~mask;
    }
}

static void image_clear_outputs(process_image *image)
{
    for (size_t i = 0; i < image->count; ++i)
        if (image->entries[i].output)
            image_set(image, &image->entries[i], 0);
}

static int image_next(process_image *image)
{
    char line[96];
    if (!fgets(line, sizeof(line), stdin) || strcmp(line, "Q\n") == 0)
        return 0;
    if (strcmp(line, "P\n") == 0 || strcmp(line, "S\n") == 0)
        return line[0];
    unsigned id;
    uint64_t value;
    char extra;
    if (sscanf(line, "W %u %" SCNu64 " %c", &id, &value, &extra) == 2 &&
        id < image->count && image->valid)
    {
        image_entry *entry = &image->entries[id];
        if (entry->output && entry->bits <= 64 &&
            (entry->bits == 64 || value < (UINT64_C(1) << entry->bits)))
        {
            image_set(image, entry, value);
            return 'P';
        }
    }
    puts("ecat\trejected\tPDO write rejected: invalid state, entry or value.");
    return 'P';
}

static void image_report(process_image *image, int valid, int wkc, int expected)
{
    image->valid = valid;
    if (!valid)
        image_clear_outputs(image);
    printf("ecat\tcycle\t%d\t%d\t%d\n", valid, wkc, expected);
    if (valid)
        for (size_t i = 0; i < image->count; ++i)
            if (image->entries[i].bits <= 64)
                printf("ecat\tvalue\t%zu\t%" PRIu64 "\n", i, image_get(image, &image->entries[i]));
    end_snapshot();
}

#endif
