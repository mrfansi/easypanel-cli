<?php

namespace App\Commands\Server;

use App\Support\ServerConfig;
use LaravelZero\Framework\Commands\Command;

class ServerAddCommand extends Command
{
    protected $signature = 'server:add {name? : Nama server} {--url= : URL host EasyPanel} {--token= : API token}';

    protected $description = 'Tambah host EasyPanel baru';

    public function handle(ServerConfig $config): int
    {
        $name = $this->argument('name') ?: $this->ask('Nama server');
        $url = $this->option('url') ?: $this->ask('URL host (mis. https://panel.example.com)');
        $token = $this->option('token') ?: $this->secret('API token');

        if (! preg_match('/^[a-z0-9-_]+$/', $name)) {
            $this->error('Nama server hanya boleh a-z, 0-9, -, _');

            return self::FAILURE;
        }

        $config->add($name, rtrim($url, '/'), $token);
        $this->info("Server '{$name}' ditambahkan.".($config->default()['name'] === $name ? ' (default)' : ''));

        return self::SUCCESS;
    }
}
