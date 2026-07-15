# EasyPanel Multi-Server CLI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** CLI Laravel Zero (`easypanel`) untuk mengelola banyak host EasyPanel — projects, services, monitoring/logs, dan node cluster.

**Architecture:** Tiga lapis: (1) `ServerConfig` menyimpan daftar host+token di `~/.config/easypanel/servers.json`; (2) `EasypanelClient` membungkus Guzzle untuk memanggil endpoint tRPC `POST {url}/api/rpc/{group}/{op}` dengan body `{"json": input}` dan unwrap `.json`; (3) command Laravel Zero yang me-resolve server aktif (default atau `--server=`) lalu memakai client.

**Tech Stack:** PHP 8.4, Laravel Zero (`laravel-zero/framework` ^12), `illuminate/http` (facade `Http`, sudah di-install via `app:install http`), Pest 3/4.

## Global Constraints

- Semua request API: `POST {url}/api/rpc/{group}/{op}`, header `Authorization: Bearer <token>`, `Content-Type: application/json`, body **selalu** `{"json": <input>}` (input `null` bila tanpa parameter — body kosong ditolak 400).
- Response sukses envelope: `{"json": <data>, "meta": [...]}` → nilai yang dipakai adalah `.json`.
- Memakai facade `Http` (illuminate/http). Test memakai `Http::fake()` + `Http::assertSent()` — tidak perlu inject client apa pun.
- Config file permission `0600`. Nama project/service tervalidasi pola `^[a-z0-9-_]+$`.
- Command auto-registered dari `app/Commands` (lihat `config/commands.php`). Namespace `App\`.
- Bahasa pesan CLI: Indonesia.
- Tidak ada test yang memanggil host asli. Verifikasi live (pakai host `https://aurel.kkbahagia.com`) hanya langkah manual read-only, plus satu siklus create→destroy project uji bernama `zzz-clitest`.

---

### Task 1: EasypanelClient + EasypanelException

**Files:**
- Create: `app/Support/EasypanelException.php`
- Create: `app/Support/EasypanelClient.php`
- Test: `tests/Unit/EasypanelClientTest.php`

**Interfaces:**
- Produces:
  - `App\Support\EasypanelException extends \RuntimeException` dengan static `fromResponse(\Illuminate\Http\Client\Response $r): self`.
  - `App\Support\EasypanelClient` — `__construct(string $url, string $token)`; `call(string $group, string $op, mixed $input = null): mixed`.

- [ ] **Step 1: Write the failing test**

```php
<?php
// tests/Unit/EasypanelClientTest.php
use App\Support\EasypanelClient;
use App\Support\EasypanelException;
use Illuminate\Support\Facades\Http;

it('posts to the rpc path with bearer auth and json envelope, unwrapping .json', function () {
    Http::fake([
        '*' => Http::response(['json' => [['name' => 'proj-a']], 'meta' => []]),
    ]);

    $result = (new EasypanelClient('https://panel.test/', 'tok123'))->call('projects', 'listProjects');

    expect($result)->toBe([['name' => 'proj-a']]);

    Http::assertSent(fn ($req) => $req->url() === 'https://panel.test/api/rpc/projects/listProjects'
        && $req->method() === 'POST'
        && $req->hasHeader('Authorization', 'Bearer tok123')
        && $req->data() === ['json' => null]);
});

it('sends the given input wrapped in json', function () {
    Http::fake(['*' => Http::response(['json' => ['ok' => true]])]);

    (new EasypanelClient('https://panel.test', 'tok123'))->call('projects', 'createProject', ['name' => 'proj-a']);

    Http::assertSent(fn ($req) => $req->data() === ['json' => ['name' => 'proj-a']]);
});

it('throws EasypanelException with a friendly message on 401', function () {
    Http::fake(['*' => Http::response(['message' => 'Unauthorized'], 401)]);

    expect(fn () => (new EasypanelClient('https://panel.test', 'tok123'))->call('projects', 'listProjects'))
        ->toThrow(EasypanelException::class, 'Token tidak valid');
});

it('throws EasypanelException surfacing the api message on other errors', function () {
    Http::fake(['*' => Http::response(['message' => 'Boom'], 500)]);

    expect(fn () => (new EasypanelClient('https://panel.test', 'tok123'))->call('projects', 'listProjects'))
        ->toThrow(EasypanelException::class, 'Boom');
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `vendor/bin/pest tests/Unit/EasypanelClientTest.php`
Expected: FAIL — `Class "App\Support\EasypanelClient" not found`.

- [ ] **Step 3: Write EasypanelException**

```php
<?php
// app/Support/EasypanelException.php
namespace App\Support;

use Illuminate\Http\Client\Response;
use RuntimeException;

class EasypanelException extends RuntimeException
{
    public static function fromResponse(Response $response): self
    {
        $status = $response->status();
        $message = $response->json('message') ?? $response->json('error') ?? $response->reason();

        if ($status === 401) {
            $message = 'Token tidak valid atau kadaluarsa (401).';
        }

        return new self("[{$status}] {$message}");
    }
}
```

- [ ] **Step 4: Write EasypanelClient**

```php
<?php
// app/Support/EasypanelClient.php
namespace App\Support;

use Illuminate\Support\Facades\Http;

class EasypanelClient
{
    public function __construct(
        private string $url,
        private string $token,
    ) {}

    /**
     * Panggil endpoint tRPC EasyPanel dan kembalikan payload `.json`.
     */
    public function call(string $group, string $op, mixed $input = null): mixed
    {
        $base = rtrim($this->url, '/');

        $response = Http::withToken($this->token)
            ->acceptJson()
            ->post("{$base}/api/rpc/{$group}/{$op}", ['json' => $input]);

        if ($response->failed()) {
            throw EasypanelException::fromResponse($response);
        }

        return $response->json('json');
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `vendor/bin/pest tests/Unit/EasypanelClientTest.php`
Expected: PASS (4 passed).

- [ ] **Step 6: Commit**

```bash
git add app/Support/EasypanelClient.php app/Support/EasypanelException.php tests/Unit/EasypanelClientTest.php
git commit -m "feat: add EasypanelClient tRPC wrapper over Http facade"
```

---

### Task 2: ServerConfig store

**Files:**
- Create: `app/Support/ServerConfig.php`
- Test: `tests/Unit/ServerConfigTest.php`

**Interfaces:**
- Produces `App\Support\ServerConfig`:
  - `__construct(string $path)`
  - `static defaultPath(): string`
  - `all(): array` — list of `['name' => string, 'url' => string, 'token' => string, 'default' => bool]`
  - `get(string $name): ?array`
  - `default(): ?array`
  - `add(string $name, string $url, string $token): void` — server pertama otomatis `default`
  - `remove(string $name): void` — jika yang dihapus adalah default & masih ada sisa, server pertama tersisa jadi default
  - `setDefault(string $name): void`

- [ ] **Step 1: Write the failing test**

```php
<?php
// tests/Unit/ServerConfigTest.php
use App\Support\ServerConfig;

beforeEach(function () {
    $this->path = sys_get_temp_dir().'/ep-cli-test-'.uniqid().'/servers.json';
});

afterEach(function () {
    @unlink($this->path);
    @rmdir(dirname($this->path));
});

it('makes the first added server the default and persists to disk', function () {
    $config = new ServerConfig($this->path);
    $config->add('prod', 'https://prod.test', 'tok-prod');

    expect((new ServerConfig($this->path))->default())
        ->toMatchArray(['name' => 'prod', 'url' => 'https://prod.test', 'default' => true]);
});

it('keeps the first server default when adding more', function () {
    $config = new ServerConfig($this->path);
    $config->add('prod', 'https://prod.test', 'tok-prod');
    $config->add('staging', 'https://staging.test', 'tok-staging');

    expect($config->default()['name'])->toBe('prod');
    expect($config->get('staging')['default'])->toBeFalse();
    expect($config->all())->toHaveCount(2);
});

it('moves the default flag with setDefault', function () {
    $config = new ServerConfig($this->path);
    $config->add('prod', 'https://prod.test', 'tok-prod');
    $config->add('staging', 'https://staging.test', 'tok-staging');
    $config->setDefault('staging');

    expect($config->default()['name'])->toBe('staging');
    expect($config->get('prod')['default'])->toBeFalse();
});

it('removes a server and reassigns default when needed', function () {
    $config = new ServerConfig($this->path);
    $config->add('prod', 'https://prod.test', 'tok-prod');
    $config->add('staging', 'https://staging.test', 'tok-staging');
    $config->remove('prod');

    expect($config->get('prod'))->toBeNull();
    expect($config->default()['name'])->toBe('staging');
});

it('writes the file with 0600 permissions', function () {
    $config = new ServerConfig($this->path);
    $config->add('prod', 'https://prod.test', 'tok-prod');

    expect(substr(sprintf('%o', fileperms($this->path)), -4))->toBe('0600');
});

it('returns empty list and null default when file is missing', function () {
    $config = new ServerConfig($this->path);

    expect($config->all())->toBe([]);
    expect($config->default())->toBeNull();
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `vendor/bin/pest tests/Unit/ServerConfigTest.php`
Expected: FAIL — `Class "App\Support\ServerConfig" not found`.

- [ ] **Step 3: Write ServerConfig**

```php
<?php
// app/Support/ServerConfig.php
namespace App\Support;

class ServerConfig
{
    public function __construct(private string $path) {}

    public static function defaultPath(): string
    {
        $home = getenv('HOME') ?: getenv('USERPROFILE') ?: sys_get_temp_dir();

        return $home.'/.config/easypanel/servers.json';
    }

    /** @return array<int, array{name:string,url:string,token:string,default:bool}> */
    public function all(): array
    {
        if (! is_file($this->path)) {
            return [];
        }

        return json_decode((string) file_get_contents($this->path), true) ?: [];
    }

    public function get(string $name): ?array
    {
        foreach ($this->all() as $server) {
            if ($server['name'] === $name) {
                return $server;
            }
        }

        return null;
    }

    public function default(): ?array
    {
        foreach ($this->all() as $server) {
            if ($server['default'] ?? false) {
                return $server;
            }
        }

        return null;
    }

    public function add(string $name, string $url, string $token): void
    {
        $servers = array_values(array_filter($this->all(), fn ($s) => $s['name'] !== $name));

        $servers[] = [
            'name' => $name,
            'url' => $url,
            'token' => $token,
            'default' => $servers === [],
        ];

        $this->save($servers);
    }

    public function remove(string $name): void
    {
        $servers = array_values(array_filter($this->all(), fn ($s) => $s['name'] !== $name));

        if ($servers !== [] && ! $this->hasDefault($servers)) {
            $servers[0]['default'] = true;
        }

        $this->save($servers);
    }

    public function setDefault(string $name): void
    {
        $servers = array_map(function ($s) use ($name) {
            $s['default'] = $s['name'] === $name;

            return $s;
        }, $this->all());

        $this->save($servers);
    }

    private function hasDefault(array $servers): bool
    {
        foreach ($servers as $s) {
            if ($s['default'] ?? false) {
                return true;
            }
        }

        return false;
    }

    private function save(array $servers): void
    {
        $dir = dirname($this->path);
        if (! is_dir($dir)) {
            mkdir($dir, 0700, true);
        }

        file_put_contents($this->path, json_encode($servers, JSON_PRETTY_PRINT));
        chmod($this->path, 0600);
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `vendor/bin/pest tests/Unit/ServerConfigTest.php`
Expected: PASS (6 passed).

- [ ] **Step 5: Commit**

```bash
git add app/Support/ServerConfig.php tests/Unit/ServerConfigTest.php
git commit -m "feat: add ServerConfig credential store"
```

---

### Task 3: Register ServerConfig + server management commands

**Files:**
- Modify: `app/Providers/AppServiceProvider.php`
- Create: `app/Commands/Server/ServerAddCommand.php`
- Create: `app/Commands/Server/ServerListCommand.php`
- Create: `app/Commands/Server/ServerUseCommand.php`
- Create: `app/Commands/Server/ServerRemoveCommand.php`

**Interfaces:**
- Consumes: `App\Support\ServerConfig` (Task 2).
- Produces: container singleton `ServerConfig` (default path); commands `server:add`, `server:list`, `server:use`, `server:remove`.

- [ ] **Step 1: Bind ServerConfig as a singleton**

Edit `app/Providers/AppServiceProvider.php` — inside `register()`:

```php
use App\Support\ServerConfig;

public function register(): void
{
    $this->app->singleton(ServerConfig::class, fn () => new ServerConfig(ServerConfig::defaultPath()));
}
```

- [ ] **Step 2: Write ServerAddCommand**

```php
<?php
// app/Commands/Server/ServerAddCommand.php
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
```

- [ ] **Step 3: Write ServerListCommand**

```php
<?php
// app/Commands/Server/ServerListCommand.php
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
```

- [ ] **Step 4: Write ServerUseCommand**

```php
<?php
// app/Commands/Server/ServerUseCommand.php
namespace App\Commands\Server;

use App\Support\ServerConfig;
use LaravelZero\Framework\Commands\Command;

class ServerUseCommand extends Command
{
    protected $signature = 'server:use {name : Nama server yang dijadikan default}';

    protected $description = 'Jadikan sebuah server sebagai default';

    public function handle(ServerConfig $config): int
    {
        $name = $this->argument('name');

        if ($config->get($name) === null) {
            $this->error("Server '{$name}' tidak ditemukan.");

            return self::FAILURE;
        }

        $config->setDefault($name);
        $this->info("Server default sekarang: {$name}");

        return self::SUCCESS;
    }
}
```

- [ ] **Step 5: Write ServerRemoveCommand**

```php
<?php
// app/Commands/Server/ServerRemoveCommand.php
namespace App\Commands\Server;

use App\Support\ServerConfig;
use LaravelZero\Framework\Commands\Command;

class ServerRemoveCommand extends Command
{
    protected $signature = 'server:remove {name : Nama server yang dihapus}';

    protected $description = 'Hapus host EasyPanel';

    public function handle(ServerConfig $config): int
    {
        $name = $this->argument('name');

        if ($config->get($name) === null) {
            $this->error("Server '{$name}' tidak ditemukan.");

            return self::FAILURE;
        }

        $config->remove($name);
        $this->info("Server '{$name}' dihapus.");

        return self::SUCCESS;
    }
}
```

- [ ] **Step 6: Verify commands work end-to-end**

Run:
```bash
php easypanel server:add aurel --url=https://aurel.kkbahagia.com --token=<TOKEN>
php easypanel server:list
php easypanel server:add second --url=https://second.test --token=abc
php easypanel server:use second
php easypanel server:list
php easypanel server:remove second
php easypanel server:list
```
Expected: `server:list` menampilkan tabel; `aurel` bertanda `*` di akhir; token tersamar; `second` hilang setelah remove.

- [ ] **Step 7: Commit**

```bash
git add app/Providers/AppServiceProvider.php app/Commands/Server
git commit -m "feat: add server management commands"
```

---

### Task 4: BaseServerCommand (server resolution + error handling)

**Files:**
- Create: `app/Commands/BaseServerCommand.php`

**Interfaces:**
- Consumes: `ServerConfig` (container), `EasypanelClient`, `EasypanelException`.
- Produces `App\Commands\BaseServerCommand extends LaravelZero\Framework\Commands\Command`:
  - adds `--server=` option to every subclass
  - `protected function client(): EasypanelClient` — resolve dari `--server=` atau default; throw `EasypanelException` bila tak ada.
  - `public function handle(): int` — panggil abstract `runServerCommand(): int` dalam try/catch `EasypanelException` (cetak `error()`, return `FAILURE`).
  - `abstract protected function runServerCommand(): int;`

- [ ] **Step 1: Write BaseServerCommand**

```php
<?php
// app/Commands/BaseServerCommand.php
namespace App\Commands;

use App\Support\EasypanelClient;
use App\Support\EasypanelException;
use App\Support\ServerConfig;
use LaravelZero\Framework\Commands\Command;
use Symfony\Component\Console\Input\InputOption;

abstract class BaseServerCommand extends Command
{
    protected function configure(): void
    {
        parent::configure();
        $this->addOption('server', null, InputOption::VALUE_REQUIRED, 'Nama server target (default: server default)');
    }

    public function handle(): int
    {
        try {
            return $this->runServerCommand();
        } catch (EasypanelException $e) {
            $this->error($e->getMessage());

            return self::FAILURE;
        }
    }

    abstract protected function runServerCommand(): int;

    protected function client(): EasypanelClient
    {
        $config = app(ServerConfig::class);
        $name = $this->option('server');
        $server = $name ? $config->get($name) : $config->default();

        if ($server === null) {
            throw new EasypanelException($name
                ? "Server '{$name}' tidak ditemukan. Lihat: easypanel server:list"
                : 'Belum ada server default. Jalankan: easypanel server:add');
        }

        return new EasypanelClient($server['url'], $server['token']);
    }
}
```

- [ ] **Step 2: Verify it loads (no syntax error)**

Run: `php easypanel list`
Expected: daftar command tampil tanpa error fatal (BaseServerCommand abstract, tidak muncul sebagai command sendiri).

- [ ] **Step 3: Commit**

```bash
git add app/Commands/BaseServerCommand.php
git commit -m "feat: add BaseServerCommand for server resolution"
```

---

### Task 5: Project commands

**Files:**
- Create: `app/Commands/Project/ProjectListCommand.php`
- Create: `app/Commands/Project/ProjectCreateCommand.php`
- Create: `app/Commands/Project/ProjectInspectCommand.php`

**Interfaces:**
- Consumes: `BaseServerCommand::client()` (Task 4).
- Produces: commands `project:list`, `project:create`, `project:inspect`.

- [ ] **Step 1: Write ProjectListCommand**

```php
<?php
// app/Commands/Project/ProjectListCommand.php
namespace App\Commands\Project;

use App\Commands\BaseServerCommand;

class ProjectListCommand extends BaseServerCommand
{
    protected $signature = 'project:list';

    protected $description = 'Daftar project di server';

    protected function runServerCommand(): int
    {
        $projects = $this->client()->call('projects', 'listProjects') ?: [];

        if ($projects === []) {
            $this->info('Tidak ada project.');

            return self::SUCCESS;
        }

        $this->table(
            ['Nama', 'Dibuat', 'Members'],
            array_map(fn ($p) => [
                $p['name'],
                $p['createdAt'] ?? '-',
                count($p['members'] ?? []),
            ], $projects),
        );

        return self::SUCCESS;
    }
}
```

- [ ] **Step 2: Write ProjectCreateCommand**

```php
<?php
// app/Commands/Project/ProjectCreateCommand.php
namespace App\Commands\Project;

use App\Commands\BaseServerCommand;

class ProjectCreateCommand extends BaseServerCommand
{
    protected $signature = 'project:create {name : Nama project (a-z 0-9 - _)}';

    protected $description = 'Buat project baru';

    protected function runServerCommand(): int
    {
        $name = $this->argument('name');

        if (! preg_match('/^[a-z0-9-_]+$/', $name)) {
            $this->error('Nama project hanya boleh a-z, 0-9, -, _');

            return self::FAILURE;
        }

        $this->client()->call('projects', 'createProject', ['name' => $name]);
        $this->info("Project '{$name}' dibuat.");

        return self::SUCCESS;
    }
}
```

- [ ] **Step 3: Write ProjectInspectCommand**

```php
<?php
// app/Commands/Project/ProjectInspectCommand.php
namespace App\Commands\Project;

use App\Commands\BaseServerCommand;

class ProjectInspectCommand extends BaseServerCommand
{
    protected $signature = 'project:inspect {name : Nama project}';

    protected $description = 'Lihat detail project dan service-nya';

    protected function runServerCommand(): int
    {
        $data = $this->client()->call('projects', 'inspectProject', ['projectName' => $this->argument('name')]);

        $services = $data['services'] ?? [];

        if ($services === []) {
            $this->info('Project tidak punya service.');

            return self::SUCCESS;
        }

        $this->table(
            ['Service', 'Tipe', 'Aktif'],
            array_map(fn ($s) => [
                $s['name'],
                $s['type'] ?? '-',
                ($s['enabled'] ?? false) ? 'ya' : 'tidak',
            ], $services),
        );

        return self::SUCCESS;
    }
}
```

- [ ] **Step 4: Verify against live host (read-only + safe create/destroy)**

Run:
```bash
php easypanel project:list --server=aurel
php easypanel project:inspect harisenin-net --server=aurel
php easypanel project:create zzz-clitest --server=aurel
php easypanel project:list --server=aurel        # zzz-clitest muncul
php easypanel call:cleanup   # (tidak ada — hapus manual di panel, atau lewat destroyProject bila ditambahkan)
```
Expected: `project:list` menampilkan tabel project asli; `project:inspect harisenin-net` menampilkan service `api`; `project:create zzz-clitest` sukses lalu muncul di list. (Bersihkan `zzz-clitest` lewat UI panel — destroy project di luar scope v1.)

- [ ] **Step 5: Commit**

```bash
git add app/Commands/Project
git commit -m "feat: add project list/create/inspect commands"
```

---

### Task 6: Service commands

**Files:**
- Create: `app/Commands/Service/ServiceDeployCommand.php`
- Create: `app/Commands/Service/ServiceRestartCommand.php`
- Create: `app/Commands/Service/ServiceStartCommand.php`
- Create: `app/Commands/Service/ServiceStopCommand.php`
- Create: `app/Commands/Service/ServiceLogsCommand.php`

**Interfaces:**
- Consumes: `BaseServerCommand::client()` (Task 4).
- Produces: commands `service:deploy`, `service:restart`, `service:start`, `service:stop`, `service:logs`.
- Konvensi: service action endpoint = group `services/{type}`, op `{action}Service`, input `{projectName, serviceName[, forceRebuild]}`. `--type` default `app`.

- [ ] **Step 1: Write ServiceDeployCommand**

```php
<?php
// app/Commands/Service/ServiceDeployCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceDeployCommand extends BaseServerCommand
{
    protected $signature = 'service:deploy {project} {service} {--type=app : Tipe service} {--force : Force rebuild}';

    protected $description = 'Deploy sebuah service';

    protected function runServerCommand(): int
    {
        $type = $this->option('type');
        $this->client()->call("services/{$type}", 'deployService', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
            'forceRebuild' => (bool) $this->option('force'),
        ]);

        $this->info("Deploy dipicu untuk {$this->argument('project')}/{$this->argument('service')}.");

        return self::SUCCESS;
    }
}
```

- [ ] **Step 2: Write ServiceRestartCommand**

```php
<?php
// app/Commands/Service/ServiceRestartCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceRestartCommand extends BaseServerCommand
{
    protected $signature = 'service:restart {project} {service} {--type=app : Tipe service}';

    protected $description = 'Restart sebuah service';

    protected function runServerCommand(): int
    {
        $type = $this->option('type');
        $this->client()->call("services/{$type}", 'restartService', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
        ]);

        $this->info("Restart dipicu untuk {$this->argument('project')}/{$this->argument('service')}.");

        return self::SUCCESS;
    }
}
```

- [ ] **Step 3: Write ServiceStartCommand**

```php
<?php
// app/Commands/Service/ServiceStartCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceStartCommand extends BaseServerCommand
{
    protected $signature = 'service:start {project} {service} {--type=app : Tipe service}';

    protected $description = 'Start sebuah service';

    protected function runServerCommand(): int
    {
        $type = $this->option('type');
        $this->client()->call("services/{$type}", 'startService', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
        ]);

        $this->info("Start dipicu untuk {$this->argument('project')}/{$this->argument('service')}.");

        return self::SUCCESS;
    }
}
```

- [ ] **Step 4: Write ServiceStopCommand**

```php
<?php
// app/Commands/Service/ServiceStopCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceStopCommand extends BaseServerCommand
{
    protected $signature = 'service:stop {project} {service} {--type=app : Tipe service}';

    protected $description = 'Stop sebuah service';

    protected function runServerCommand(): int
    {
        $type = $this->option('type');
        $this->client()->call("services/{$type}", 'stopService', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
        ]);

        $this->info("Stop dipicu untuk {$this->argument('project')}/{$this->argument('service')}.");

        return self::SUCCESS;
    }
}
```

- [ ] **Step 5: Write ServiceLogsCommand**

```php
<?php
// app/Commands/Service/ServiceLogsCommand.php
namespace App\Commands\Service;

use App\Commands\BaseServerCommand;

class ServiceLogsCommand extends BaseServerCommand
{
    protected $signature = 'service:logs {project} {service} {--limit=100 : Jumlah baris log}';

    protected $description = 'Tampilkan log service';

    protected function runServerCommand(): int
    {
        $lines = $this->client()->call('logs', 'queryServiceLogs', [
            'projectName' => $this->argument('project'),
            'serviceName' => $this->argument('service'),
            'limit' => (int) $this->option('limit'),
        ]) ?: [];

        // Bentuk log bisa berupa list string atau list objek; tangani keduanya.
        foreach ((is_array($lines) ? $lines : [$lines]) as $line) {
            if (is_string($line)) {
                $this->line($line);
            } elseif (is_array($line)) {
                $this->line($line['message'] ?? $line['line'] ?? json_encode($line));
            }
        }

        return self::SUCCESS;
    }
}
```

- [ ] **Step 6: Verify against live host**

Run:
```bash
php easypanel service:logs harisenin-net api --server=aurel --limit=20
php easypanel service:restart harisenin-net api --server=aurel   # (opsional; memicu restart nyata)
```
Expected: `service:logs` mencetak baris log tanpa error. (Jalankan `service:restart` hanya bila aman memicu restart service nyata.)

- [ ] **Step 7: Commit**

```bash
git add app/Commands/Service
git commit -m "feat: add service deploy/restart/start/stop/logs commands"
```

---

### Task 7: Monitoring (stats) + cluster (node:list)

**Files:**
- Create: `app/Commands/StatsCommand.php`
- Create: `app/Commands/Node/NodeListCommand.php`

**Interfaces:**
- Consumes: `BaseServerCommand::client()` (Task 4).
- Produces: commands `stats`, `node:list`.

- [ ] **Step 1: Write StatsCommand**

```php
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
```

- [ ] **Step 2: Write NodeListCommand**

```php
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
            ['Node', 'Detail'],
            array_map(fn ($n) => is_array($n)
                ? [$n['name'] ?? $n['hostname'] ?? '-', json_encode($n)]
                : [$n, ''], $nodes),
        );

        return self::SUCCESS;
    }
}
```

- [ ] **Step 3: Verify against live host**

Run:
```bash
php easypanel stats --server=aurel
php easypanel node:list --server=aurel
```
Expected: `stats` menampilkan tabel CPU/mem/disk/uptime dengan angka nyata; `node:list` menampilkan node atau pesan "tidak ada node".

- [ ] **Step 4: Run full test suite**

Run: `vendor/bin/pest`
Expected: semua unit test (Task 1 & 2) PASS.

- [ ] **Step 5: Commit**

```bash
git add app/Commands/StatsCommand.php app/Commands/Node
git commit -m "feat: add stats and node:list commands"
```

---

## Self-Review

**Spec coverage:**
- Config store lewat command → Task 2 (ServerConfig) + Task 3 (server:add/list/use/remove). ✓
- `EasypanelClient` contract (POST, Bearer, `{json:input}`, unwrap `.json`) → Task 1. ✓
- `BaseServerCommand` resolve `--server`/default + error handling → Task 4. ✓
- Projects (list/create/inspect) → Task 5. ✓
- Services (deploy/restart/start/stop) + `--type` → Task 6. ✓
- Monitoring `stats` + `service:logs` → Task 6 (logs) & Task 7 (stats). ✓
- `node:list` → Task 7. ✓
- Error handling tanpa stack trace, exit non-zero → Task 4 handle(). ✓
- Testing minimal (client + config, tanpa host asli) → Task 1 & 2. ✓

**Placeholder scan:** Tidak ada TBD/TODO; semua step berisi kode lengkap. Verifikasi command memakai host asli sebagai langkah manual (bukan automated test), sesuai scope spec.

**Type consistency:** `client()` / `call($group,$op,$input)` / `ServerConfig` method names konsisten di seluruh task. Service commands memakai group `services/{type}` seragam. `runServerCommand()` diimplementasikan tiap command konkret; `handle()` hanya di base.

**Catatan dependency:** Komponen `illuminate/http` di-install lewat `php easypanel app:install http` (sudah dilakukan & di-commit terpisah) agar facade `Http` + `Http::fake()` tersedia — sesuai spec.
