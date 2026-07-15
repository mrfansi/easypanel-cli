<?php
// app/Commands/Node/NodeListCommand.php
namespace App\Commands\Node;

use App\Commands\BaseServerCommand;

class NodeListCommand extends BaseServerCommand
{
    protected $signature = 'node:list';

    protected $description = 'Daftar node cluster host';

    protected function runServerCommand(): int
    {
        $nodes = $this->client()->call('cluster', 'listNodes') ?: [];

        if (! is_array($nodes) || $nodes === []) {
            $this->info('Tidak ada node (atau host bukan cluster).');

            return self::SUCCESS;
        }

        $this->table(
            ['Hostname', 'Role', 'State', 'Availability', 'Addr'],
            array_map(fn ($n) => [
                data_get($n, 'Description.Hostname', '-'),
                data_get($n, 'Spec.Role', '-'),
                data_get($n, 'Status.State', '-'),
                data_get($n, 'Spec.Availability', '-'),
                data_get($n, 'Status.Addr', '-'),
            ], $nodes),
        );

        return self::SUCCESS;
    }
}
