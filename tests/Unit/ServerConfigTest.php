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
