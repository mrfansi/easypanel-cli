<?php
// app/Commands/MenuCommand.php
namespace App\Commands;

use App\Support\EasypanelClient;
use App\Support\EasypanelException;
use App\Support\LogFormatter;
use App\Support\ServerConfig;
use LaravelZero\Framework\Commands\Command;

class MenuCommand extends Command
{
    protected $signature = 'menu';

    protected $description = 'Menu interaktif untuk kelola EasyPanel';

    private ServerConfig $config;

    public function handle(ServerConfig $config): int
    {
        $this->config = $config;

        if ($config->all() === []) {
            $this->warn('Belum ada server. Jalankan: easypanel server:add');

            return self::FAILURE;
        }

        do {
            $server = $this->pickServer();

            if ($server === null) {
                break;
            }

            $this->serverMenu($server);
            // Dengan satu server, keluar dari server menu = keluar aplikasi.
        } while (count($this->config->all()) > 1);

        return self::SUCCESS;
    }

    private function pickServer(): ?array
    {
        $servers = $this->config->all();

        if (count($servers) === 1) {
            return $servers[0];
        }

        $options = [];
        foreach ($servers as $s) {
            $options[$s['name']] = $s['name'].(($s['default'] ?? false) ? ' (default)' : '').' — '.$s['url'];
        }

        $name = $this->styledMenu('Pilih server', $options)->setExitButtonText('Keluar')->open();

        return $name === null ? null : $this->config->get($name);
    }

    private function serverMenu(array $server): void
    {
        $client = new EasypanelClient($server['url'], $server['token']);

        while (true) {
            $exitLabel = count($this->config->all()) > 1 ? 'Ganti server' : 'Keluar';
            $choice = $this->styledMenu("Server: {$server['name']}", [
                'projects' => 'Projects',
                'stats' => 'Monitoring (system stats)',
                'nodes' => 'Node cluster',
            ])->setExitButtonText($exitLabel)->open();

            if ($choice === null) {
                return;
            }

            $this->guard(fn () => match ($choice) {
                'projects' => $this->projectsMenu($client),
                'stats' => $this->showStats($client),
                'nodes' => $this->showNodes($client),
                default => null,
            });
        }
    }

    private function projectsMenu(EasypanelClient $client): void
    {
        while (true) {
            $projects = $client->call('projects', 'listProjects') ?: [];

            if ($projects === []) {
                $this->info('Tidak ada project.');

                return;
            }

            $options = [];
            foreach ($projects as $p) {
                $options[$p['name']] = $p['name'];
            }

            $name = $this->styledMenu('Pilih project', $options)->setExitButtonText('Kembali')->open();

            if ($name === null) {
                return;
            }

            $this->guard(fn () => $this->servicesMenu($client, $name));
        }
    }

    private function servicesMenu(EasypanelClient $client, string $project): void
    {
        while (true) {
            $data = $client->call('projects', 'inspectProject', ['projectName' => $project]);
            $services = $data['services'] ?? [];

            if ($services === []) {
                $this->info('Project tanpa service.');

                return;
            }

            $options = [];
            foreach ($services as $s) {
                $type = $s['type'] ?? 'app';
                // Kunci menggabungkan nama+tipe (dipisah '|') karena aksi butuh keduanya.
                $options[$s['name'].'|'.$type] = $s['name'].' ('.$type.')';
            }

            $choice = $this->styledMenu("Project: {$project}", $options)->setExitButtonText('Kembali')->open();

            if ($choice === null) {
                return;
            }

            [$service, $type] = explode('|', $choice, 2);
            $this->guard(fn () => $this->serviceActionMenu($client, $project, $service, $type));
        }
    }

    private function serviceActionMenu(EasypanelClient $client, string $project, string $service, string $type): void
    {
        while (true) {
            $action = $this->styledMenu("{$project} / {$service} ({$type})", [
                'deploy' => 'Deploy',
                'restart' => 'Restart',
                'start' => 'Start',
                'stop' => 'Stop',
                'logs' => 'Lihat logs (100 baris)',
            ])->setExitButtonText('Kembali')->open();

            if ($action === null) {
                return;
            }

            $this->guard(fn () => $this->runServiceAction($client, $project, $service, $type, $action));
        }
    }

    private function runServiceAction(EasypanelClient $client, string $project, string $service, string $type, string $action): void
    {
        if ($action === 'logs') {
            $result = $client->call('logs', 'queryServiceLogs', [
                'projectName' => $project,
                'serviceName' => $service,
                'limit' => 100,
            ]) ?: [];

            $lines = LogFormatter::format($result);
            $lines === [] ? $this->info('Tidak ada log.') : array_walk($lines, fn ($l) => $this->line($l));

            return;
        }

        // Aksi yang memengaruhi service nyata butuh konfirmasi.
        if (in_array($action, ['deploy', 'restart', 'stop'], true)
            && ! $this->confirm(ucfirst($action)." '{$service}' pada '{$project}'? Ini memengaruhi service nyata.", $action === 'deploy')) {
            return;
        }

        $input = ['projectName' => $project, 'serviceName' => $service];
        if ($action === 'deploy') {
            $input['forceRebuild'] = false;
        }

        $client->call("services/{$type}", "{$action}Service", $input);
        $this->info(ucfirst($action)." dipicu untuk {$project}/{$service}.");
    }

    private function showStats(EasypanelClient $client): void
    {
        $s = $client->call('monitorOld', 'getSystemStats') ?: [];

        $this->table(['Metrik', 'Nilai'], [
            ['CPU cores', data_get($s, 'cpuInfo.count', '-')],
            ['CPU used %', data_get($s, 'cpuInfo.usedPercentage', '-')],
            ['Mem used %', data_get($s, 'memInfo.usedMemPercentage', '-')],
            ['Mem used MB', data_get($s, 'memInfo.usedMemMb', '-')],
            ['Disk used %', data_get($s, 'diskInfo.usedPercentage', '-')],
            ['Disk free GB', data_get($s, 'diskInfo.freeGb', '-')],
            ['Uptime (s)', data_get($s, 'uptime', '-')],
        ]);
    }

    private function showNodes(EasypanelClient $client): void
    {
        $nodes = $client->call('cluster', 'listNodes') ?: [];

        if (! is_array($nodes) || $nodes === []) {
            $this->info('Tidak ada node (atau host bukan cluster).');

            return;
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
    }

    /** Bangun menu dengan skema warna kontras-tinggi (latar hitam, teks putih) untuk tema terminal gelap. */
    private function styledMenu(string $title, array $options = [])
    {
        return $this->menu($title, $options)
            ->setBackgroundColour('black')
            ->setForegroundColour('white');
    }

    /** Jalankan aksi menu; tampilkan error API dengan rapi tanpa keluar dari menu. */
    private function guard(callable $fn): void
    {
        try {
            $fn();
        } catch (EasypanelException $e) {
            $this->error($e->getMessage());
            $this->newLine();
        }
    }
}
