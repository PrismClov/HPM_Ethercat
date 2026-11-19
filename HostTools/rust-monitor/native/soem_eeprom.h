#ifndef MONITOR_SOEM_EEPROM_H
#define MONITOR_SOEM_EEPROM_H

/* Hardware SII read with an explicit WKC result, including the all-zero case.
 * SOEM's ecx_readeepromFP value alone cannot distinguish zero from failure. */
static int sii_ready(ecx_contextt *context, uint16_t addr, uint16_t *status)
{
    for (unsigned i = 0; i < 100; ++i)
    {
        uint16_t raw = 0;
        if (ecx_FPRD(&context->port, addr, ECT_REG_EEPSTAT, sizeof(raw), &raw, EC_TIMEOUTRET) == 1 &&
            !(etohs(raw) & EC_ESTAT_BUSY))
        {
            *status = etohs(raw);
            return 1;
        }
        osal_usleep(1000);
    }
    return 0;
}

static int sii_word(ecx_contextt *context, uint16_t addr, uint16_t word, uint16_t *value)
{
    uint16_t status;
    if (!sii_ready(context, addr, &status))
        return 0;
    if (status & EC_ESTAT_EMASK)
    {
        uint16_t nop = htoes(EC_ECMD_NOP);
        if (ecx_FPWR(&context->port, addr, ECT_REG_EEPCTL, sizeof(nop), &nop, EC_TIMEOUTRET) != 1)
            return 0;
    }
    uint16_t cmd[3] = {htoes(EC_ECMD_READ), htoes(word), 0};
    uint16_t data = 0;
    if (ecx_FPWR(&context->port, addr, ECT_REG_EEPCTL, sizeof(cmd), cmd, EC_TIMEOUTRET) != 1 ||
        !sii_ready(context, addr, &status) || (status & EC_ESTAT_EMASK) ||
        ecx_FPRD(&context->port, addr, ECT_REG_EEPDAT, sizeof(data), &data, EC_TIMEOUTRET) != 1)
        return 0;
    *value = etohs(data);
    return 1;
}

static int soem_eeprom(ecx_contextt *context, int count, unsigned position)
{
    if (count <= 0 || position >= (unsigned)count)
    {
        pdo_error("EEPROM position is not on the scanned bus.");
        return 1;
    }
    unsigned index = position + 1;
    ec_slavet *slave = &context->slavelist[index];
    int state, code;
    read_al(context, slave, &state, &code);
    if (state != EC_STATE_INIT || code)
    {
        pdo_error("EEPROM service requires the target in INIT without AL error.");
        return 1;
    }
    uint16_t addr = slave->configadr;
    uint8_t force = 2, master = 0;
    int result = 1;
    uint16_t size;
    if (ecx_FPWR(&context->port, addr, ECT_REG_EEPCFG, sizeof(force), &force, EC_TIMEOUTRET) != 1 ||
        ecx_FPWR(&context->port, addr, ECT_REG_EEPCFG, sizeof(master), &master, EC_TIMEOUTRET) != 1 ||
        !sii_word(context, addr, 0x3e, &size) || size >= 1024)
    {
        pdo_error("Cannot take EEPROM ownership or read a supported capacity.");
        goto release;
    }
    unsigned capacity = ((unsigned)size + 1) * 64;
    printf("ecat\tcapacity\t%u\n", capacity);
    end_snapshot();
    char line[96];
    while (fgets(line, sizeof(line), stdin))
    {
        unsigned word, before, after;
        char extra;
        uint16_t value;
        if (sscanf(line, "R %u %c", &word, &extra) == 1 && word < capacity &&
            sii_word(context, addr, (uint16_t)word, &value))
        {
            printf("ecat\tword\t%u\t%u\n", word, value);
            end_snapshot();
        }
        else if (sscanf(line, "W %u %u %u %c", &word, &before, &after, &extra) == 3 &&
                 word >= 0x40 && word < capacity && before <= UINT16_MAX && after <= UINT16_MAX)
        {
            read_al(context, slave, &state, &code);
            if (state != EC_STATE_INIT || code ||
                !sii_word(context, addr, (uint16_t)word, &value) || value != before ||
                !ecx_writeeepromFP(context, addr, (uint16_t)word, htoes((uint16_t)after), 200000) ||
                !sii_word(context, addr, (uint16_t)word, &value) || value != after)
            {
                printf("ecat\terror\tEEPROM write or verification failed at word 0x%04x.\n", word);
                fflush(stdout);
                goto release;
            }
            printf("ecat\tword\t%u\t%u\n", word, value);
            end_snapshot();
        }
        else
        {
            pdo_error("Invalid or failed EEPROM request.");
            goto release;
        }
    }
    result = 0;
release:
    slave->eep_pdi = 0;
    if (ecx_eeprom2pdi(context, (uint16_t)index) <= 0 && !result)
        result = 1;
    return result;
}

#endif
