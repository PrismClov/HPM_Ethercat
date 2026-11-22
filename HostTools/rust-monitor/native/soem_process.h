/* SPDX-License-Identifier: GPL-3.0-only */
#ifndef MONITOR_SOEM_PROCESS_H
#define MONITOR_SOEM_PROCESS_H

#include "process_image.h"

static int soem_object(ecx_contextt *ctx, unsigned slave, uint16_t index,
                       uint8_t subindex, unsigned bytes, uint32_t *value)
{
    uint8_t data[4] = {0};
    int size = sizeof(data);
    if (ecx_SDOread(ctx, (uint16_t)slave, index, subindex, FALSE, &size, data, EC_TIMEOUTRXM) <= 0 ||
        (size != (int)bytes && !(bytes == 1 && size == 2 && data[1] == 0)))
        return 0;
    *value = hpm_u32(data);
    return 1;
}

static unsigned sii_u16(ecx_contextt *ctx, unsigned station, unsigned address)
{
    unsigned lo = ecx_siigetbyte(ctx, (uint16_t)station, (uint16_t)address);
    return lo | (unsigned)ecx_siigetbyte(ctx, (uint16_t)station, (uint16_t)(address + 1)) << 8;
}

static int soem_sii_mapping(ecx_contextt *ctx, process_image *image, unsigned station,
                            unsigned output, unsigned sm, size_t offset, unsigned total)
{
    int section = ecx_siifind(ctx, (uint16_t)station, output ? 51 : 50);
    if (section <= 0)
        return 0;
    unsigned pos = (unsigned)section + 2;
    unsigned end = pos + 2 * sii_u16(ctx, station, (unsigned)section);
    unsigned used = 0;
    if (end > UINT16_MAX)
        return 0;
    while (pos < end)
    {
        if (end - pos < 8)
            return 0;
        unsigned pdo = sii_u16(ctx, station, pos);
        unsigned entries = ecx_siigetbyte(ctx, (uint16_t)station, (uint16_t)(pos + 2));
        unsigned manager = ecx_siigetbyte(ctx, (uint16_t)station, (uint16_t)(pos + 3));
        pos += 8;
        if (entries > (end - pos) / 8)
            return 0;
        for (unsigned i = 0; i < entries; ++i, pos += 8)
        {
            if (sm != manager)
                continue;
            unsigned index = sii_u16(ctx, station, pos);
            unsigned sub = ecx_siigetbyte(ctx, (uint16_t)station, (uint16_t)(pos + 2));
            unsigned bits = ecx_siigetbyte(ctx, (uint16_t)station, (uint16_t)(pos + 5));
            if (!bits || bits > total - used ||
                !image_add(image, station - 1, output, pdo, index, sub, bits, offset + used, NULL))
                return 0;
            used += bits;
        }
    }
    return used == total;
}

/* Port of SimpleCAT tc3_coe_read_pdos: assignments -> PDOs -> bit entries.
 * The SOEM transport owns the real IOmap; estimated offsets are accepted only
 * when the online mapping covers exactly one SM for each process direction. */
static int soem_map_direction(ecx_contextt *ctx, process_image *image, unsigned station,
                              unsigned output, uint16_t assignment)
{
    ec_slavet *slave = &ctx->slavelist[station];
    unsigned total = output ? slave->Obits : slave->Ibits;
    const uint8_t *data = output ? slave->outputs : slave->inputs;
    if (!total)
        return 1;
    unsigned matching = 0, manager = 0;
    for (unsigned sm = 0; sm < EC_MAXSM; ++sm)
        if (slave->SMtype[sm] == (output ? 3 : 4) && etohs(slave->SM[sm].SMlength))
        {
            ++matching;
            manager = sm;
        }
    if (matching != 1 || !data)
        return 0;
    size_t offset = (size_t)(data - image->data) * 8 + (output ? slave->Ostartbit : slave->Istartbit);
    if (!(slave->mbx_proto & ECT_MBXPROT_COE))
        return soem_sii_mapping(ctx, image, station, output, manager, offset, total);
    uint32_t count, pdo, entries, entry;
    if (!soem_object(ctx, station, assignment, 0, 1, &count) || count > 255)
        return 0;
    unsigned used = 0;
    for (unsigned i = 1; i <= count; ++i)
    {
        if (!soem_object(ctx, station, assignment, (uint8_t)i, 2, &pdo) ||
            !pdo || !soem_object(ctx, station, (uint16_t)pdo, 0, 1, &entries) || entries > 255)
            return 0;
        for (unsigned j = 1; j <= entries; ++j)
        {
            if (!soem_object(ctx, station, (uint16_t)pdo, (uint8_t)j, 4, &entry))
                return 0;
            unsigned bits = entry & 0xff;
            if (!bits || bits > total - used ||
                !image_add(image, station - 1, output, pdo, entry >> 16,
                           (entry >> 8) & 0xff, bits, offset + used, NULL))
                return 0;
            used += bits;
        }
    }
    return used == total;
}

static int soem_process(ecx_contextt *ctx, int count, uint32_t period_ns, unsigned assign)
{
    int result = 1;
    if (count <= 0 || (assign && assign != 0x0300))
    {
        pdo_error("SOEM needs responding slaves and supports AssignActivate 0x0300 for SYNC0.");
        return 1;
    }
    process_image *image = calloc(1, sizeof(*image));
    /* SOEM's per-slave byte counts fit uint16 bit lengths. Reserve room for
     * both directions and mailbox status before calling its unsized mapper. */
    uint8_t *map = calloc((size_t)count, 16386u);
    if (!image || !map)
    {
        pdo_error("Cannot allocate SOEM process image.");
        goto done;
    }
    ctx->slavelist[0].state = EC_STATE_PRE_OP;
    if (ecx_writestate(ctx, 0) <= 0 ||
        ecx_statecheck(ctx, 0, EC_STATE_PRE_OP, EC_TIMEOUTSTATE) != EC_STATE_PRE_OP)
    {
        pdo_error("SOEM slaves did not enter PRE-OP.");
        goto stop;
    }
    int size = ecx_config_map_group(ctx, map, 0);
    ec_groupt *group = &ctx->grouplist[0];
    int expected = group->outputsWKC * 2 + group->inputsWKC;
    if (size <= 0 || (size_t)size > (size_t)count * 16386u || expected <= 0)
    {
        pdo_error("SOEM did not create a valid process image.");
        goto stop;
    }
    image->data = map;
    image->bytes = (size_t)size;
    for (int i = 1; i <= count; ++i)
    {
        if (!soem_map_direction(ctx, image, (unsigned)i, 1, 0x1c12) ||
            !soem_map_direction(ctx, image, (unsigned)i, 0, 0x1c13))
        {
            pdo_error("SOEM cannot verify a slave's active PDO layout against its IOmap.");
            goto stop;
        }
    }
    if (assign)
    {
        if (!ecx_configdc(ctx))
        {
            pdo_error("SOEM did not discover a DC-capable slave.");
            goto stop;
        }
        for (int i = 1; i <= count; ++i)
            if (!ctx->slavelist[i].hasdc)
            {
                pdo_error("A slave in the requested DC group lacks a distributed clock.");
                goto stop;
            }
        for (int i = 1; i <= count; ++i)
            ecx_dcsync0(ctx, (uint16_t)i, TRUE, period_ns, 0);
    }
    ctx->slavelist[0].state = EC_STATE_SAFE_OP;
    if (ecx_writestate(ctx, 0) <= 0 ||
        ecx_statecheck(ctx, 0, EC_STATE_SAFE_OP, EC_TIMEOUTSTATE) != EC_STATE_SAFE_OP)
    {
        pdo_error("SOEM slaves did not enter SAFE-OP.");
        goto stop;
    }
    /* SOEM schedules the first SYNC0 at least 100 ms in the future.
     * Keep zero PDOs circulating in SAFE-OP while both clocks start. */
    uint64_t next = monotonic_ns();
    uint64_t ready = next + (assign ? UINT64_C(100000000) + 2u * period_ns : 0);
    do
    {
        ecx_send_processdata(ctx);
        ecx_receive_processdata(ctx, EC_TIMEOUTRET);
        if (!assign)
            break;
        next += period_ns;
        uint64_t now = monotonic_ns();
        if (next > now)
            osal_usleep((uint32_t)((next - now) / 1000));
        else
            next = now;
    } while (next < ready);
    ctx->slavelist[0].state = EC_STATE_OPERATIONAL;
    ecx_writestate(ctx, 0);
    if (!image_layout(image))
    {
        pdo_error("SOEM PDO layout has no valid entries.");
        goto stop;
    }
    int timeout_us = (int)(period_ns / 1000u);
    if (timeout_us > EC_TIMEOUTRET)
        timeout_us = EC_TIMEOUTRET;
    int operational = 0;
    for (int req; (req = image_next(image)) != 0;)
    {
        int wkc = ecx_send_processdata(ctx) > 0 ? ecx_receive_processdata(ctx, timeout_us) : EC_NOFRAME;
        if (req == 'S' || !image->valid)
        {
            operational = bus_count(ctx) == count;
            for (int i = 1; i <= count; ++i)
            {
                int state, code;
                read_al(ctx, &ctx->slavelist[i], &state, &code);
                printf("ecat\tal\t%d\t%d\t%d\n", i - 1, state, code);
                operational = operational && state == EC_STATE_OPERATIONAL && !code;
            }
        }
        if (assign && operational && wkc == expected)
            printf("ecat\tdc\t%" PRId64 "\n", (int64_t)ctx->DCtime);
        image_report(image, operational && wkc == expected, wkc, expected);
    }
    image_clear_outputs(image);
    ecx_send_processdata(ctx);
    ecx_receive_processdata(ctx, EC_TIMEOUTRET);
    result = 0;
stop:
    ctx->slavelist[0].state = EC_STATE_SAFE_OP;
    ecx_writestate(ctx, 0);
    if (assign)
        for (int i = 1; i <= count; ++i)
            if (ctx->slavelist[i].DCactive)
                ecx_dcsync0(ctx, (uint16_t)i, FALSE, 0, 0);
done:
    free(map);
    free(image);
    return result;
}

#endif
