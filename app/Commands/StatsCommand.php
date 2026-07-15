<?php
// app/Commands/StatsCommand.php
namespace App\Commands;

class StatsCommand extends BaseServerCommand
{
    protected $signature = 'stats';

    protected $description = 'System stats host (CPU, memori, disk, uptime)';

    protected function runServerCommand(): int
    {
        $s = $this->client()->call('monitorOld', 'getSystemStats') ?: [];

        $cpu = $s['cpuInfo'] ?? [];
        $mem = $s['memInfo'] ?? [];
        $disk = $s['diskInfo'] ?? [];

        $this->table(['Metrik', 'Nilai'], [
            ['CPU cores', $cpu['count'] ?? '-'],
            ['CPU used %', $cpu['usedPercentage'] ?? '-'],
            ['Mem used %', $mem['usedMemPercentage'] ?? '-'],
            ['Mem used MB', $mem['usedMemMb'] ?? '-'],
            ['Disk used %', $disk['usedPercentage'] ?? '-'],
            ['Disk free GB', $disk['freeGb'] ?? '-'],
            ['Uptime (s)', $s['uptime'] ?? '-'],
        ]);

        return self::SUCCESS;
    }
}
