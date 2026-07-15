<?php

namespace App\Commands\Server;

use App\Support\ServerConfig;
use LaravelZero\Framework\Commands\Command;

class ServerListCommand extends Command
{
    protected $signature = 'server:list';

    protected $description = 'Daftar host EasyPanel terkonfigurasi';

    public function handle(ServerConfig $config): int
    {
        $servers = $config->all();

        if ($servers === []) {
            $this->warn('Belum ada server. Jalankan: easypanel server:add');

            return self::SUCCESS;
        }

        $this->table(
            ['Default', 'Nama', 'URL', 'Token'],
            array_map(fn ($s) => [
                ($s['default'] ?? false) ? '*' : '',
                $s['name'],
                $s['url'],
                substr($s['token'], 0, 6).'…'.substr($s['token'], -4),
            ], $servers),
        );

        return self::SUCCESS;
    }
}
